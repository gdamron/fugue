//! Tests for the spectrum analyser.

use super::*;
use crate::spectrum::SpectrumTap;

mod timeline;
use timeline::{frames_in, Timeline};

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
        SpectrumConfig {
            fft_size: 8_192,
            hop_size: 4_096,
            ..config()
        },
        SpectrumConfig {
            history_frames: MAX_HISTORY_FRAMES + 1,
            ..config()
        },
        SpectrumConfig {
            // Would ask for terabytes when a tile is allocated.
            max_frames_per_tile: u32::MAX,
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

/// Runs the analyser over everything waiting and checks it produced exactly
/// the frames the timeline says it should: every frame whose window arrived
/// whole, at its true position, and nothing stitched across a loss.
fn assert_frames_match(timeline: &Timeline, tiles: &[SpectrogramTile], config: &SpectrumConfig) {
    let expected = timeline.expected_frames(config.fft_size as u64, config.hop_size as u64);
    let produced = frames_in(tiles);
    if produced != expected {
        let first_difference = produced
            .iter()
            .zip(&expected)
            .position(|(p, e)| p != e)
            .unwrap_or(produced.len().min(expected.len()));
        panic!(
            "produced {} frames, expected {}; first difference at index {first_difference}: \
             produced {:?}, expected {:?} (lost {} samples)",
            produced.len(),
            expected.len(),
            produced.get(first_difference),
            expected.get(first_difference),
            timeline.lost()
        );
    }
    let bins = config.fft_size / 2 + 1;
    for tile in tiles {
        assert!(tile.frame_count <= config.max_frames_per_tile);
        assert_eq!(
            tile.magnitudes_db.len(),
            tile.frame_count as usize * bins,
            "tile {} is misshapen",
            tile.tile_seq
        );
    }
}

/// The feature's headline claim: a stall leaves a gap exactly where the audio
/// was lost, and every frame after it keeps its true position.
#[test]
fn places_the_gap_where_the_audio_was_lost() {
    let (tap, mut analyzer) = analyzer(config());
    let mut timeline = Timeline::new(&tap);
    let mut tiles = Vec::new();

    for _ in 0..40 {
        timeline.feed(1_024);
        tiles.extend(drain(&mut analyzer));
    }
    // Overrun: far more than the ring holds, with nobody draining.
    timeline.feed(200_000);
    for _ in 0..40 {
        timeline.feed(1_024);
        tiles.extend(drain(&mut analyzer));
    }

    assert!(timeline.lost() > 0, "the tap should have overrun");
    assert_frames_match(&timeline, &tiles, &config());
}

/// Stopping and restarting analysis on the same invention is designed in, so
/// a second stream must place its gaps by its own audio, not the tap's life.
#[test]
fn a_later_stream_on_the_same_tap_places_its_gaps_correctly() {
    let tap = SpectrumTap::new();
    let mut first =
        SpectrumAnalyzer::new(tap.take_reader().unwrap(), RATE, "first", config()).unwrap();
    for _ in 0..50 {
        feed_silence(&tap, 1_024);
        drain(&mut first);
    }
    drop(first);

    let mut second = SpectrumAnalyzer::new(
        tap.take_reader().expect("returned"),
        RATE,
        "second",
        config(),
    )
    .unwrap();
    let mut timeline = Timeline::new(&tap);
    let mut tiles = Vec::new();
    for _ in 0..10 {
        timeline.feed(1_024);
        tiles.extend(drain(&mut second));
    }
    timeline.feed(100_000);
    for _ in 0..20 {
        timeline.feed(1_024);
        tiles.extend(drain(&mut second));
    }

    assert!(timeline.lost() > 0);
    assert_frames_match(&timeline, &tiles, &config());
}

/// Under sustained overload the analyser falls behind again and again before
/// it reaches its first gap: it holds one loss while more queue up behind it.
/// Audio accepted between losses must keep its place, and no frame may be
/// stitched across any of them.
#[test]
fn keeps_audio_between_losses_in_its_place() {
    let (tap, mut analyzer) = analyzer(config());
    let mut timeline = Timeline::new(&tap);
    let mut tiles = Vec::new();

    for _ in 0..8 {
        timeline.feed(1_024);
        tiles.extend(drain(&mut analyzer));
    }
    // Stall, make a little progress, stall again — four times over, so the
    // analyser is holding one loss while later ones are still queued.
    timeline.feed(60_000);
    for _ in 0..4 {
        tiles.extend(analyzer.poll());
        timeline.feed(2_048);
    }
    timeline.feed(60_000);
    for _ in 0..30 {
        timeline.feed(1_024);
        tiles.extend(drain(&mut analyzer));
    }

    assert!(timeline.lost() > 0);
    assert_frames_match(&timeline, &tiles, &config());
}

/// When audio after a gap is already waiting as the analyser reaches it, the
/// first frame past the gap must start a new tile rather than continue the
/// one before it.
#[test]
fn starts_a_new_tile_at_a_gap_even_mid_pass() {
    let (tap, mut analyzer) = analyzer(config());
    let mut timeline = Timeline::new(&tap);
    let mut tiles = Vec::new();

    for _ in 0..8 {
        timeline.feed(1_024);
        tiles.extend(drain(&mut analyzer));
    }
    timeline.feed(100_000);
    tiles.extend(analyzer.poll());
    timeline.feed(1_024); // audio past the gap, waiting before we reach it
    tiles.extend(drain(&mut analyzer));

    assert!(
        tiles
            .windows(2)
            .any(|pair| pair[1].start_frame > pair[0].start_frame + pair[0].frame_count as u64),
        "the scenario should produce a gap"
    );
    assert_frames_match(&timeline, &tiles, &config());
}

/// Frame numbering follows position, so repeated stalls cannot accumulate an
/// offset; at four-times overlap, where counting drifts fastest, every frame
/// still matches the audio that actually arrived.
#[test]
fn repeated_stalls_do_not_accumulate_drift() {
    let config = SpectrumConfig {
        fft_size: 512,
        hop_size: 128,
        max_frames_per_tile: 8,
        ..Default::default()
    };
    let tap = SpectrumTap::new();
    let mut analyzer =
        SpectrumAnalyzer::new(tap.take_reader().unwrap(), RATE, "drift", config.clone()).unwrap();
    let mut timeline = Timeline::new(&tap);
    let mut tiles = Vec::new();

    for _ in 0..6 {
        timeline.feed(60_000);
        for _ in 0..8 {
            timeline.feed(2_048);
            tiles.extend(drain(&mut analyzer));
        }
        assert_frames_match(&timeline, &tiles, &config);
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
