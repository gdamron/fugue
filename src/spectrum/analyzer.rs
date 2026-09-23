use super::tap::SpectrumReader;
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
/// Frame numbers come from each frame's position in the stream rather than
/// from counting frames as they are produced, so they cannot drift away from
/// wall-clock time however often analysis stalls. Audio the tap had to drop
/// leaves a gap exactly where it was lost: frames whose windows would straddle
/// the loss are never produced, rather than stitched across it.
///
/// All buffers are allocated up front; a pass allocates only the tile it
/// returns.
pub struct SpectrumAnalyzer {
    reader: SpectrumReader,
    meta: SpectrogramStreamMeta,
    config: SpectrumConfig,
    fft: RealFft,
    /// Window coefficients, one per transform point.
    window: Vec<f32>,
    /// Scale from raw magnitude to amplitude, given the window's gain.
    amplitude_scale: f32,
    /// The most recent `fft_size` samples, oldest first.
    frame: Vec<f32>,
    /// How much of `frame` is filled while the stream starts or restarts.
    filled: usize,
    /// Windowed copy handed to the transform.
    windowed: Vec<f32>,
    /// Raw magnitudes from the transform.
    magnitudes: Vec<f32>,
    /// Samples pulled from the tap in one read.
    scratch: Vec<f32>,
    /// Samples taken from the tap, including any discarded.
    consumed: u64,
    /// Samples lost to holes and already accounted for on the time axis.
    skipped: u64,
    /// Samples to discard before frames line up with the hop grid again.
    align_debt: usize,
    /// A hole the tap reported that the analyser has not reached yet.
    hole: Option<(u64, u64)>,
    /// A completed frame held back because it does not continue the tile
    /// being built; it starts the next one.
    ready_frame: Option<u64>,
    next_tile_seq: u64,
}

impl SpectrumAnalyzer {
    /// Builds an analyser for `reader`, and starts collection.
    pub fn new(
        mut reader: SpectrumReader,
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

        reader.start();
        Ok(Self {
            reader,
            meta,
            fft,
            window,
            amplitude_scale,
            frame: vec![0.0; config.fft_size],
            filled: 0,
            windowed: vec![0.0; config.fft_size],
            magnitudes: vec![0.0; bin_count],
            // Sized for the largest single read, which is a whole frame while
            // the stream starts or restarts.
            scratch: vec![0.0; config.fft_size],
            consumed: 0,
            skipped: 0,
            align_debt: 0,
            hole: None,
            ready_frame: None,
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
    /// A tile holds consecutive frames only. Where audio was lost the tile
    /// ends, and the next one resumes at the first frame past the gap.
    pub fn poll(&mut self) -> Option<SpectrogramTile> {
        let bin_count = self.meta.frequency.bin_count as usize;
        let max_frames = self.config.max_frames_per_tile as usize;
        let mut magnitudes_db: Vec<f32> = Vec::new();
        let mut start_frame = 0u64;
        let mut frames = 0usize;

        while frames < max_frames {
            let Some(index) = self
                .ready_frame
                .take()
                .or_else(|| self.advance_to_next_frame())
            else {
                break;
            };
            if frames == 0 {
                start_frame = index;
                magnitudes_db.reserve(max_frames * bin_count);
            } else if index != start_frame + frames as u64 {
                // A gap: this frame belongs to the next tile, and its audio is
                // still in `frame` for that pass to analyse.
                self.ready_frame = Some(index);
                break;
            }
            self.analyze_current_frame(&mut magnitudes_db);
            frames += 1;
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
    pub fn stop(&mut self) {
        self.reader.stop();
    }

    /// Where the next sample to be read sits in the stream, counting audio
    /// that was lost as well as audio that arrived.
    fn absolute_position(&self) -> u64 {
        self.consumed + self.skipped
    }

    /// Fills the analysis frame until one is complete, returning its index on
    /// the stream's hop grid, or `None` when the tap has run dry.
    fn advance_to_next_frame(&mut self) -> Option<u64> {
        let size = self.config.fft_size;
        let hop = self.config.hop_size;

        loop {
            if self.hole.is_none() {
                self.hole = self.reader.take_hole();
            }
            if let Some((at, len)) = self.hole {
                if self.consumed >= at {
                    self.apply_hole(len);
                    continue;
                }
            }
            if self.align_debt > 0 {
                let wanted = self.align_debt;
                if !self.discard(wanted) {
                    return None;
                }
                self.align_debt -= wanted;
                continue;
            }

            let needed = if self.filled < size {
                size - self.filled
            } else {
                hop
            };
            // Never read across a hole: the audio beyond it is not continuous
            // with what this frame already holds.
            if let Some((at, _)) = self.hole {
                let room = (at - self.consumed) as usize;
                if room < needed {
                    // Too little audio left before the loss to finish a frame,
                    // so this run ends here.
                    if !self.discard(room) {
                        return None;
                    }
                    continue;
                }
            }
            if self.reader.available() < needed {
                return None;
            }
            let taken = self.reader.read_samples(&mut self.scratch[..needed]);
            debug_assert_eq!(taken, needed, "availability was just checked");
            self.consumed += taken as u64;

            if self.filled < size {
                self.frame[self.filled..self.filled + taken]
                    .copy_from_slice(&self.scratch[..taken]);
                self.filled += taken;
            } else {
                self.frame.copy_within(hop.., 0);
                self.frame[size - hop..].copy_from_slice(&self.scratch[..hop]);
            }

            // The frame covers the `size` samples ending here, and the hop
            // grid is kept aligned, so its index follows from its position.
            let start = self.absolute_position() - size as u64;
            debug_assert_eq!(start % hop as u64, 0, "frames stay on the hop grid");
            return Some(start / hop as u64);
        }
    }

    /// Accounts for `len` lost samples at the point they were lost, and lines
    /// the next frame up with the hop grid on the far side of the gap.
    fn apply_hole(&mut self, len: u64) {
        self.skipped += len;
        self.hole = None;
        self.filled = 0;
        let misaligned = self.absolute_position() % self.config.hop_size as u64;
        self.align_debt = if misaligned == 0 {
            0
        } else {
            (self.config.hop_size as u64 - misaligned) as usize
        };
    }

    /// Throws away `count` samples, reporting whether they were all there.
    fn discard(&mut self, count: usize) -> bool {
        if count == 0 {
            return true;
        }
        if self.reader.available() < count {
            return false;
        }
        let taken = self.reader.read_samples(&mut self.scratch[..count]);
        self.consumed += taken as u64;
        taken == count
    }

    /// Windows the current frame, transforms it, and appends its decibels.
    fn analyze_current_frame(&mut self, out: &mut Vec<f32>) {
        for (i, sample) in self.frame.iter().enumerate() {
            self.windowed[i] = sample * self.window[i];
        }
        self.fft.magnitudes(&self.windowed, &mut self.magnitudes);
        let floor = self.config.floor_db;
        let last_bin = self.magnitudes.len() - 1;
        for (bin, magnitude) in self.magnitudes.iter().enumerate() {
            // Doubling accounts for a real signal's energy sitting at both the
            // positive and the negative frequency. DC and Nyquist have no
            // mirror image, so doubling them would read 6 dB high.
            let scale = if bin == 0 || bin == last_bin {
                0.5 * self.amplitude_scale
            } else {
                self.amplitude_scale
            };
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
mod tests;
