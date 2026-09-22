use super::tap::SpectrumTap;
use crate::dsp::fft::RealFft;
use crate::rpc::{
    SpectrogramDbReference, SpectrogramDbScale, SpectrogramEncoding, SpectrogramFrequencyAxis,
    SpectrogramLimits, SpectrogramProvenance, SpectrogramStreamMeta, SpectrogramTile,
    SpectrogramWindow,
};
use std::f32::consts::PI;

/// How a spectrogram stream is analysed. Defaults suit a display-rate view of
/// the master output: about 94 frames a second at 48 kHz, 513 bins, and a
/// history of a few seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumConfig {
    /// Transform size in samples; must be a power of two.
    pub fft_size: usize,
    /// Samples between consecutive frames.
    pub hop_size: usize,
    pub window: SpectrogramWindow,
    /// Quietest level reported; anything quieter is clamped here.
    pub floor_db: f32,
    /// Loudest level a viewer needs to distinguish.
    pub ceiling_db: f32,
    /// Frames a viewer is expected to keep.
    pub history_frames: u32,
    /// Largest tile the analyser will emit.
    pub max_frames_per_tile: u32,
    /// Which signal is analysed, as it appears to a reader.
    pub source: String,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        Self {
            fft_size: 1024,
            hop_size: 512,
            window: SpectrogramWindow::Hann,
            floor_db: -100.0,
            ceiling_db: 0.0,
            history_frames: 512,
            max_frames_per_tile: 8,
            source: "sink:master".to_string(),
        }
    }
}

impl SpectrumConfig {
    /// Checks the settings a stream cannot recover from.
    pub fn validate(&self) -> Result<(), String> {
        if self.fft_size < 32 || !self.fft_size.is_power_of_two() {
            return Err("fft_size must be a power of two of at least 32".to_string());
        }
        if self.hop_size == 0 || self.hop_size > self.fft_size {
            return Err("hop_size must be 1..=fft_size".to_string());
        }
        if self.floor_db >= self.ceiling_db {
            return Err("floor_db must be below ceiling_db".to_string());
        }
        if self.history_frames == 0 || self.max_frames_per_tile == 0 {
            return Err("history_frames and max_frames_per_tile must be positive".to_string());
        }
        Ok(())
    }
}

/// Turns tapped audio into spectrogram tiles.
///
/// Built once per stream and driven from a control thread — never the audio
/// thread. Each pass drains whatever the tap holds and emits tiles for the
/// frames that completed, so the caller decides the cadence by how often it
/// calls [`poll`](Self::poll).
///
/// All buffers are allocated up front. A pass allocates only the tile it
/// returns.
pub struct SpectrumAnalyzer {
    tap: SpectrumTap,
    meta: SpectrogramStreamMeta,
    config: SpectrumConfig,
    fft: RealFft,
    /// Window coefficients, one per transform point.
    window: Vec<f32>,
    /// Scale from raw magnitude to amplitude, given the window's gain.
    amplitude_scale: f32,
    /// The most recent `fft_size` samples, oldest first.
    frame: Vec<f32>,
    /// How much of `frame` is filled while the stream starts up.
    filled: usize,
    /// Windowed copy handed to the transform.
    windowed: Vec<f32>,
    /// Raw magnitudes from the transform.
    magnitudes: Vec<f32>,
    /// Samples pulled from the tap in one read.
    scratch: Vec<f32>,
    /// Absolute index of the next frame to be produced.
    next_frame: u64,
    next_tile_seq: u64,
}

impl SpectrumAnalyzer {
    /// Builds an analyser for `tap`, and enables collection.
    pub fn new(
        tap: SpectrumTap,
        sample_rate: u32,
        stream_id: impl Into<String>,
        config: SpectrumConfig,
    ) -> Result<Self, String> {
        config.validate()?;
        if sample_rate == 0 {
            return Err("sample_rate must be positive".to_string());
        }
        let fft = RealFft::new(config.fft_size);
        let window = window_coefficients(config.window, config.fft_size);
        // A sine at a bin centre puts amplitude * gain * size / 2 in its bin,
        // where gain is the window's mean. Undo that, so a full-scale sine
        // reads 0 dBFS whatever the window and size.
        let window_sum: f32 = window.iter().sum();
        let amplitude_scale = 2.0 / window_sum.max(f32::MIN_POSITIVE);
        let bin_count = fft.bin_count();

        let meta = SpectrogramStreamMeta {
            stream_id: stream_id.into(),
            provenance: SpectrogramProvenance {
                sample_rate,
                fft_size: config.fft_size as u32,
                hop_size: config.hop_size as u32,
                window: config.window,
                source: config.source.clone(),
            },
            frequency: SpectrogramFrequencyAxis {
                min_hz: 0.0,
                bin_hz: sample_rate as f32 / config.fft_size as f32,
                bin_count: bin_count as u32,
            },
            db: SpectrogramDbScale {
                reference: SpectrogramDbReference::Dbfs,
                floor_db: config.floor_db,
                ceiling_db: config.ceiling_db,
            },
            limits: SpectrogramLimits {
                history_frames: config.history_frames,
                max_frames_per_tile: config.max_frames_per_tile,
            },
            encoding: SpectrogramEncoding::F32Json,
        };

        tap.set_enabled(true);
        Ok(Self {
            tap,
            meta,
            fft,
            window,
            amplitude_scale,
            frame: vec![0.0; config.fft_size],
            filled: 0,
            windowed: vec![0.0; config.fft_size],
            magnitudes: vec![0.0; bin_count],
            scratch: vec![0.0; config.hop_size],
            next_frame: 0,
            next_tile_seq: 0,
            config,
        })
    }

    /// The stream's fixed metadata, to announce before any tile.
    pub fn meta(&self) -> &SpectrogramStreamMeta {
        &self.meta
    }

    /// Analyses whatever audio is waiting and returns one tile, or `None` when
    /// no frame completed. Call again while it keeps returning tiles.
    ///
    /// Audio dropped because analysis fell behind advances the frame numbering
    /// without producing frames, so viewers see a gap at the right place
    /// instead of a seamless jump.
    pub fn poll(&mut self) -> Option<SpectrogramTile> {
        self.skip_dropped_audio();

        let bin_count = self.meta.frequency.bin_count as usize;
        let max_frames = self.config.max_frames_per_tile as usize;
        let mut magnitudes_db: Vec<f32> = Vec::new();
        let mut frames = 0usize;
        let start_frame = self.next_frame;

        while frames < max_frames {
            if !self.advance_one_hop() {
                break;
            }
            if magnitudes_db.is_empty() {
                magnitudes_db.reserve(max_frames * bin_count);
            }
            self.analyze_current_frame(&mut magnitudes_db);
            frames += 1;
            self.next_frame += 1;
        }

        if frames == 0 {
            return None;
        }
        let tile = SpectrogramTile {
            stream_id: self.meta.stream_id.clone(),
            tile_seq: self.next_tile_seq,
            start_frame,
            frame_count: frames as u32,
            magnitudes_db,
        };
        self.next_tile_seq += 1;
        Some(tile)
    }

    /// Stops collection. The tap can be analysed again by a later stream.
    pub fn stop(&self) {
        self.tap.set_enabled(false);
    }

    /// Accounts for audio the tap had to drop: the frames it would have filled
    /// are skipped, so later frames keep their true position in time.
    fn skip_dropped_audio(&mut self) {
        let dropped = self.tap.take_dropped();
        if dropped == 0 {
            return;
        }
        // The partly filled frame is no longer continuous with what follows.
        self.filled = 0;
        let hop = self.config.hop_size as u64;
        self.next_frame += dropped.div_ceil(hop);
    }

    /// Slides the analysis frame forward by one hop, if the tap has the audio.
    fn advance_one_hop(&mut self) -> bool {
        let hop = self.config.hop_size;
        let size = self.config.fft_size;
        // Starting up, the frame needs a whole transform's worth before the
        // first hop can complete.
        let needed = if self.filled < size {
            (size - self.filled).min(hop.max(size - self.filled))
        } else {
            hop
        };
        if self.tap.available() < needed {
            return false;
        }
        if self.scratch.len() < needed {
            self.scratch.resize(needed, 0.0);
        }
        let taken = self.tap.read_samples(&mut self.scratch[..needed]);
        if taken < needed {
            return false;
        }
        if self.filled < size {
            // Fill from the front while the stream starts.
            let room = size - self.filled;
            let count = taken.min(room);
            self.frame[self.filled..self.filled + count].copy_from_slice(&self.scratch[..count]);
            self.filled += count;
            return self.filled == size;
        }
        self.frame.copy_within(hop.., 0);
        self.frame[size - hop..].copy_from_slice(&self.scratch[..hop]);
        true
    }

    /// Windows the current frame, transforms it, and appends its decibels.
    fn analyze_current_frame(&mut self, out: &mut Vec<f32>) {
        for (i, sample) in self.frame.iter().enumerate() {
            self.windowed[i] = sample * self.window[i];
        }
        self.fft.magnitudes(&self.windowed, &mut self.magnitudes);
        let floor = self.config.floor_db;
        let scale = self.amplitude_scale;
        for magnitude in &self.magnitudes {
            let amplitude = magnitude * scale;
            let db = if amplitude > 0.0 {
                20.0 * amplitude.log10()
            } else {
                floor
            };
            out.push(db.max(floor));
        }
    }
}

/// Coefficients for one analysis window.
fn window_coefficients(window: SpectrogramWindow, size: usize) -> Vec<f32> {
    let denominator = (size - 1).max(1) as f32;
    (0..size)
        .map(|i| {
            let t = i as f32 / denominator;
            match window {
                SpectrogramWindow::Rectangular => 1.0,
                SpectrogramWindow::Hann => 0.5 - 0.5 * (2.0 * PI * t).cos(),
                SpectrogramWindow::Hamming => 0.54 - 0.46 * (2.0 * PI * t).cos(),
                SpectrogramWindow::Blackman => {
                    0.42 - 0.5 * (2.0 * PI * t).cos() + 0.08 * (4.0 * PI * t).cos()
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let analyzer = SpectrumAnalyzer::new(tap.clone(), RATE, "test", config).unwrap();
        (tap, analyzer)
    }

    /// Feeds `frames` samples of a sine at `hz`, peaking at `amplitude`.
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

    /// Loudest bin of a tile's first frame.
    fn peak_bin(tile: &SpectrogramTile, bin_count: usize) -> usize {
        let frame = &tile.magnitudes_db[..bin_count];
        frame
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
        assert!(SpectrumAnalyzer::new(SpectrumTap::new(), 0, "s", config()).is_err());
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

        let mut tile = analyzer.poll().unwrap();
        // Skip the start-up frame, which is only partly filled by the tone.
        tile = analyzer.poll().unwrap_or(tile);
        assert_eq!(peak_bin(&tile, 129), bin);

        let peak = tile.magnitudes_db[bin];
        assert!(
            (peak - 0.0).abs() < 0.5,
            "a full-scale sine should read about 0 dBFS, got {peak}"
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
            analyzer.poll();
            let tile = analyzer.poll().unwrap();
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
        assert_eq!(first.frame_count, 4);
        assert_eq!(first.start_frame, 0);
        assert_eq!(first.tile_seq, 0);
        assert_eq!(first.magnitudes_db.len(), 4 * 129);
        assert!(first.matches(analyzer.meta()));

        let second = analyzer.poll().unwrap();
        assert_eq!(second.start_frame, 4);
        assert_eq!(second.tile_seq, 1);
        assert!(second.frame_count <= 4);
    }

    #[test]
    fn numbers_frames_continuously_across_polls() {
        let (tap, mut analyzer) = analyzer(config());
        let mut expected = 0u64;
        for _ in 0..6 {
            feed_silence(&tap, 128 * 4);
            while let Some(tile) = analyzer.poll() {
                assert_eq!(tile.start_frame, expected);
                expected += tile.frame_count as u64;
            }
        }
        assert!(expected > 10, "only {expected} frames");
    }

    #[test]
    fn skips_frame_numbers_over_audio_the_tap_dropped() {
        let (tap, mut analyzer) = analyzer(config());
        feed_silence(&tap, 512);
        while analyzer.poll().is_some() {}
        let before = analyzer.next_frame;

        // Overrun the tap, then keep feeding.
        let flood = vec![0.0; 40_000];
        tap.observe_block(&flood, &flood, flood.len());
        let dropped = tap.dropped();
        assert!(dropped > 0, "the tap should have overrun");

        while analyzer.poll().is_some() {}
        let hop = 128u64;
        assert!(
            analyzer.next_frame >= before + dropped.div_ceil(hop),
            "dropped audio must advance the time axis"
        );
    }

    #[test]
    fn stops_collecting_when_told() {
        let (tap, analyzer) = analyzer(config());
        assert!(tap.is_enabled());
        analyzer.stop();
        assert!(!tap.is_enabled());
    }
}
