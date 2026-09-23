//! Tests for the spectrum analyser.

use super::*;
use crate::spectrum::SpectrumTap;

const RATE: u32 = 48_000;

fn config() -> SpectrumConfig {
    SpectrumConfig {
        fft_size: 256,
        hop_size: 128,
        max_frames_per_tile: 4,
        ..Default::default()
    }
}

fn analyzer(config: SpectrumConfig) -> (SpectrumTap, SpectrumAnalyzer) {
    let tap = SpectrumTap::new();
    let reader = tap.take_reader().unwrap();
    let analyzer = SpectrumAnalyzer::new(reader, RATE, "test", config).unwrap();
    (tap, analyzer)
}

/// Feeds `samples` of a sine at `hz`, peaking at `amplitude`.
fn feed_tone(tap: &SpectrumTap, hz: f32, amplitude: f32, samples: usize, phase: &mut f32) {
    let block: Vec<f32> = (0..samples)
        .map(|_| {
            let value = amplitude * (*phase).sin();
            *phase += 2.0 * PI * hz / RATE as f32;
            value
        })
        .collect();
    tap.observe_block(&block, &block, block.len());
}

fn feed_silence(tap: &SpectrumTap, samples: usize) {
    let block = vec![0.0; samples];
    tap.observe_block(&block, &block, block.len());
}

fn feed_constant(tap: &SpectrumTap, value: f32, samples: usize) {
    let block = vec![value; samples];
    tap.observe_block(&block, &block, block.len());
}

/// Every tile the analyser will currently produce.
fn drain(analyzer: &mut SpectrumAnalyzer) -> Vec<SpectrogramTile> {
    let mut tiles = Vec::new();
    while let Some(tile) = analyzer.poll() {
        tiles.push(tile);
    }
    tiles
}

/// Loudest bin of a tile's first frame.
fn peak_bin(tile: &SpectrogramTile, bin_count: usize) -> usize {
    tile.magnitudes_db[..bin_count]
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(bin, _)| bin)
        .unwrap()
}

#[test]
fn describes_its_own_analysis() {
    let (_tap, analyzer) = analyzer(config());
    let meta = analyzer.meta();
    assert_eq!(meta.stream_id, "test");
    assert_eq!(meta.provenance.fft_size, 256);
    assert_eq!(meta.provenance.hop_size, 128);
    assert_eq!(meta.frequency.bin_count, 129);
    assert_eq!(meta.frequency.bin_hz, RATE as f32 / 256.0);
    assert_eq!(meta.encoding, SpectrogramEncoding::F32Json);
}

#[test]
fn rejects_settings_a_stream_cannot_recover_from() {
    for bad in [
        SpectrumConfig {
            fft_size: 300,
            ..config()
        },
        SpectrumConfig {
            hop_size: 0,
            ..config()
        },
        SpectrumConfig {
            hop_size: 512,
            fft_size: 256,
            ..config()
        },
        SpectrumConfig {
            floor_db: 0.0,
            ceiling_db: -100.0,
            ..config()
        },
        SpectrumConfig {
            history_frames: 0,
            ..config()
        },
    ] {
        assert!(bad.validate().is_err(), "accepted {bad:?}");
    }
    let tap = SpectrumTap::new();
    assert!(SpectrumAnalyzer::new(tap.take_reader().unwrap(), 0, "s", config()).is_err());
}

#[test]
fn produces_nothing_until_a_whole_frame_is_available() {
    let (tap, mut analyzer) = analyzer(config());
    feed_silence(&tap, 255);
    assert!(analyzer.poll().is_none());
    feed_silence(&tap, 1);
    let tile = analyzer.poll().expect("a full frame completed");
    assert_eq!(tile.start_frame, 0);
    assert_eq!(tile.frame_count, 1);
}

#[test]
fn reports_silence_at_the_floor() {
    let (tap, mut analyzer) = analyzer(config());
    feed_silence(&tap, 1024);
    let tile = analyzer.poll().unwrap();
    assert!(tile.magnitudes_db.iter().all(|db| *db == -100.0));
    assert!(tile.magnitudes_db.iter().all(|db| db.is_finite()));
}

#[test]
fn puts_a_tone_in_its_own_bin_at_the_right_level() {
    let (tap, mut analyzer) = analyzer(config());
    let bin = 20;
    let hz = bin as f32 * RATE as f32 / 256.0;
    let mut phase = 0.0;
    feed_tone(&tap, hz, 1.0, 4096, &mut phase);

    analyzer.poll().expect("the start-up frame");
    let tile = analyzer.poll().expect("a steady-state frame");
    assert_eq!(peak_bin(&tile, 129), bin);

    let peak = tile.magnitudes_db[bin];
    assert!(
        peak.abs() < 0.5,
        "a full-scale sine should read about 0 dBFS, got {peak}"
    );
}

#[test]
fn does_not_overstate_dc_or_nyquist() {
    // Both bins lack a mirror image, so the doubling a real sine needs
    // would report them 6 dB high.
    let (dc_tap, mut dc_analyzer) = analyzer(config());
    feed_constant(&dc_tap, 1.0, 4096);
    dc_analyzer.poll().expect("the start-up frame");
    let dc = dc_analyzer
        .poll()
        .expect("a steady-state frame")
        .magnitudes_db[0];
    assert!(dc.abs() < 0.5, "full-scale DC should read 0 dBFS, got {dc}");

    let (nyquist_tap, mut nyquist_analyzer) = analyzer(config());
    let alternating: Vec<f32> = (0..4096)
        .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    nyquist_tap.observe_block(&alternating, &alternating, alternating.len());
    nyquist_analyzer.poll().expect("the start-up frame");
    let tile = nyquist_analyzer.poll().expect("a steady-state frame");
    let nyquist = tile.magnitudes_db[128];
    assert!(
        nyquist.abs() < 0.5,
        "full-scale Nyquist should read 0 dBFS, got {nyquist}"
    );
}

#[test]
fn scales_levels_with_amplitude() {
    let mut levels = Vec::new();
    for amplitude in [1.0, 0.5, 0.25] {
        let (tap, mut analyzer) = analyzer(config());
        let mut phase = 0.0;
        feed_tone(
            &tap,
            20.0 * RATE as f32 / 256.0,
            amplitude,
            4096,
            &mut phase,
        );
        analyzer.poll().expect("the start-up frame");
        let tile = analyzer.poll().expect("a steady-state frame");
        levels.push(tile.magnitudes_db[20]);
    }
    // Halving amplitude is 6 dB down, twice over.
    assert!((levels[0] - levels[1] - 6.0).abs() < 0.5, "{levels:?}");
    assert!((levels[1] - levels[2] - 6.0).abs() < 0.5, "{levels:?}");
}

#[test]
fn fills_tiles_up_to_the_declared_limit() {
    let (tap, mut analyzer) = analyzer(config());
    feed_silence(&tap, 256 + 128 * 9);
    let first = analyzer.poll().unwrap();
    assert_eq!(first.frame_count, 4, "a full tile is four frames here");
    assert_eq!(first.start_frame, 0);
    assert_eq!(first.tile_seq, 0);
    assert_eq!(first.magnitudes_db.len(), 4 * 129);
    assert!(first.matches(analyzer.meta()));

    let second = analyzer.poll().unwrap();
    assert_eq!(second.start_frame, 4, "tiles continue where the last ended");
    assert_eq!(second.tile_seq, 1);
    assert_eq!(second.frame_count, 4);
}

#[test]
fn numbers_frames_by_position_in_the_stream() {
    let (tap, mut analyzer) = analyzer(config());
    let hop = 128u64;
    let mut fed = 0u64;
    let mut expected = 0u64;
    for _ in 0..6 {
        feed_silence(&tap, 128 * 4);
        fed += 128 * 4;
        for tile in drain(&mut analyzer) {
            assert_eq!(tile.start_frame, expected);
            expected += tile.frame_count as u64;
        }
        // Every frame whose window has arrived in full, and no more.
        let complete = (fed.saturating_sub(256) / hop) + 1;
        assert_eq!(expected, complete, "after {fed} samples");
    }
}

/// The feature's headline claim: a stall must leave a gap where the audio
/// was actually lost, and frames after it must keep their true position.
#[test]
fn places_the_gap_where_the_audio_was_lost() {
    let (tap, mut analyzer) = analyzer(config());
    let hop = 128u64;
    let mut tiles = Vec::new();

    // A stretch of ordinary audio, analysed as it arrives.
    let clean_before = 40 * 1_024u64;
    for _ in 0..40 {
        feed_silence(&tap, 1_024);
        tiles.extend(drain(&mut analyzer));
    }
    assert!(
        !tiles.is_empty(),
        "the clean audio should have been analysed"
    );
    assert_eq!(tap.dropped_total(), 0, "no loss before the stall");

    // Now overrun: one huge block with nobody draining. The ring keeps
    // what it can; the rest is lost *after* that accepted audio.
    let flood = 200_000u64;
    let block = vec![0.0; flood as usize];
    tap.observe_block(&block, &block, block.len());
    let dropped_at_flood = tap.dropped_total();
    assert!(dropped_at_flood > 0, "the tap should have overrun");
    let accepted = flood - dropped_at_flood;

    // More ordinary audio afterwards.
    for _ in 0..40 {
        feed_silence(&tap, 1_024);
        tiles.extend(drain(&mut analyzer));
    }
    let total_fed = clean_before + flood + 40 * 1_024;
    // The ring stays full for the first feeds after the flood, so a little
    // more audio is lost at the same point; it belongs to the same gap.
    let dropped = tap.dropped_total();
    assert!(dropped >= dropped_at_flood);

    // Find where the frame numbering jumps: that is the gap.
    let mut gap = None;
    for pair in tiles.windows(2) {
        let end = pair[0].start_frame + pair[0].frame_count as u64;
        if pair[1].start_frame > end {
            assert!(gap.is_none(), "one stall should make one gap");
            gap = Some((end, pair[1].start_frame));
        }
    }
    let (gap_start, gap_end) = gap.expect("the lost audio should leave a gap");

    // The audio the ring accepted before the loss is continuous with what
    // came before it, so the gap opens only after that audio — not where
    // the analyser first heard about the loss.
    let expected_start = (clean_before + accepted) / hop;
    assert!(
        gap_start.abs_diff(expected_start) <= 2,
        "gap opens at frame {gap_start}, but the loss began at frame {expected_start}"
    );
    // It is as wide as the audio that went missing, plus the frames whose
    // windows would have straddled the loss — those are rightly not
    // produced rather than stitched across it.
    let span = gap_end - gap_start;
    let straddling = (256 / hop) + 1;
    assert!(
        span >= dropped / hop && span <= dropped / hop + straddling + 1,
        "gap spans {span} frames for {dropped} lost samples (hop {hop})"
    );

    // Time still tracks the audio that was fed, gap included.
    let last = tiles.last().unwrap();
    let end_frame = last.start_frame + last.frame_count as u64;
    assert!(
        end_frame.abs_diff(total_fed / hop) <= 4,
        "frame {end_frame} against {} frames of audio fed",
        total_fed / hop
    );
}

/// Frame numbering is derived from position, so repeated stalls cannot
/// accumulate an offset the way incremental counting does.
#[test]
fn repeated_stalls_do_not_accumulate_drift() {
    // Four-times overlap, where incremental counting drifts fastest.
    let config = SpectrumConfig {
        fft_size: 512,
        hop_size: 128,
        max_frames_per_tile: 8,
        ..Default::default()
    };
    let (tap, mut analyzer) = analyzer(config);
    let hop = 128u64;
    let mut fed = 0u64;
    let mut last_end = 0u64;

    for round in 0..6 {
        // Stall: feed far more than the ring holds without draining.
        let flood = vec![0.0; 60_000];
        tap.observe_block(&flood, &flood, flood.len());
        fed += 60_000;
        // Then a spell of ordinary, drained audio.
        for _ in 0..8 {
            feed_silence(&tap, 2_048);
            fed += 2_048;
            for tile in drain(&mut analyzer) {
                last_end = tile.start_frame + tile.frame_count as u64;
            }
        }
        let fed_frames = fed / hop;
        assert!(
            last_end <= fed_frames + 1,
            "round {round}: frame {last_end} runs ahead of the {fed_frames} frames fed"
        );
        // Position-derived numbering tracks the audio actually fed; a
        // counter that lost frames per stall would fall steadily behind.
        assert!(
            last_end + 64 >= fed_frames,
            "round {round}: frame {last_end} lags the {fed_frames} frames fed"
        );
    }
}

#[test]
fn a_tile_never_spans_a_gap() {
    let (tap, mut analyzer) = analyzer(config());
    feed_silence(&tap, 4_096);
    drain(&mut analyzer);

    let flood = vec![0.0; 100_000];
    tap.observe_block(&flood, &flood, flood.len());
    for tile in drain(&mut analyzer) {
        // Frames within a tile are consecutive by construction; the tile
        // ends at a gap rather than stitching across it.
        assert_eq!(
            tile.magnitudes_db.len(),
            tile.frame_count as usize * 129,
            "tile {} is misshapen",
            tile.tile_seq
        );
        assert!(tile.matches(analyzer.meta()));
    }
}

#[test]
fn stops_collecting_when_told() {
    let (tap, mut analyzer) = analyzer(config());
    assert!(tap.is_enabled());
    analyzer.stop();
    assert!(!tap.is_enabled());
}

#[test]
fn dropping_the_analyser_stops_collecting() {
    let (tap, analyzer) = analyzer(config());
    assert!(tap.is_enabled());
    drop(analyzer);
    assert!(
        !tap.is_enabled(),
        "an abandoned analyser must not leave the audio thread collecting"
    );
    assert!(
        tap.take_reader().is_some(),
        "and it must give the reading end back"
    );
}
