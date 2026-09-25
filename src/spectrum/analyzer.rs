use super::config::{window_coefficients, SpectrumConfig};
use super::tap::SpectrumReader;
use crate::dsp::RealFft;
use crate::rpc::{
    SpectrogramBinSpacing, SpectrogramDbReference, SpectrogramDbScale, SpectrogramEncoding,
    SpectrogramFrequencyAxis, SpectrogramLimits, SpectrogramMagnitudes, SpectrogramProvenance,
    SpectrogramStreamMeta, SpectrogramTile,
};
use base64::Engine as _;
use std::ops::Range;

/// Turns tapped audio into spectrogram tiles.
///
/// Built once per stream and driven from a control thread — never the audio
/// thread. Each pass drains whatever the reader holds and emits tiles for the
/// frames that completed, so the caller decides the cadence by how often it
/// calls [`poll`](Self::poll).
///
/// Frame numbers come from each frame's position in the stream rather than
/// from counting frames as they are produced, so they cannot drift away from
/// wall-clock time however often analysis stalls. Audio the reader lost leaves
/// a gap exactly where it was lost: frames whose windows would straddle the
/// loss are never produced, rather than stitched across it.
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
    /// Scale from raw power to squared amplitude, for bins with a mirror
    /// image and for DC and Nyquist, which have none.
    power_scale: f32,
    edge_power_scale: f32,
    /// Squared amplitude at the floor; anything at or below it reads the
    /// floor without a logarithm being taken.
    floor_power: f32,
    /// The most recent `fft_size` samples, oldest first.
    frame: Vec<f32>,
    /// How much of `frame` holds samples of the frame being built.
    filled: usize,
    /// Windowed copy handed to the transform.
    windowed: Vec<f32>,
    /// Power per bin from the transform.
    power: Vec<f32>,
    /// Samples pulled from the reader; `staged` is the part not yet used.
    scratch: Vec<f32>,
    staged: Range<usize>,
    /// Samples to discard before frames line up with the hop grid again.
    align_debt: usize,
    /// A completed frame held back because it does not continue the tile
    /// being built; it starts the next one.
    ready_frame: Option<u64>,
    /// Decibel levels of the tile being built, reused from tile to tile.
    levels: Vec<f32>,
    next_tile_seq: u64,
    /// Every loss met, as (stream position, length), for tests to check
    /// frames against.
    #[cfg(test)]
    losses: Vec<(u64, u64)>,
}

impl SpectrumAnalyzer {
    /// Builds an analyser reading from `reader`.
    pub fn new(
        reader: SpectrumReader,
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
        // reads 0 dBFS whatever the window and size. Doubling accounts for a
        // real signal's energy sitting at both the positive and the negative
        // frequency; DC and Nyquist have no mirror image, so they are not
        // doubled.
        let window_sum: f32 = window.iter().sum();
        let amplitude_scale = 2.0 / window_sum.max(f32::MIN_POSITIVE);
        let edge_amplitude_scale = 0.5 * amplitude_scale;
        let bin_count = fft.bin_count();

        let meta = SpectrogramStreamMeta {
            stream_id: stream_id.into(),
            provenance: SpectrogramProvenance {
                sample_rate,
                fft_size: config.fft_size as u32,
                hop_size: config.hop_size as u32,
                window: config.window,
                source: reader.source().to_string(),
            },
            frequency: SpectrogramFrequencyAxis {
                spacing: SpectrogramBinSpacing::Linear,
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
            encoding: config.encoding,
        };

        Ok(Self {
            reader,
            meta,
            fft,
            window,
            power_scale: amplitude_scale * amplitude_scale,
            edge_power_scale: edge_amplitude_scale * edge_amplitude_scale,
            floor_power: 10f32.powf(config.floor_db / 10.0),
            frame: vec![0.0; config.fft_size],
            filled: 0,
            windowed: vec![0.0; config.fft_size],
            power: vec![0.0; bin_count],
            scratch: vec![0.0; config.fft_size],
            staged: 0..0,
            align_debt: 0,
            ready_frame: None,
            levels: Vec::with_capacity(config.max_frames_per_tile as usize * bin_count),
            next_tile_seq: 0,
            #[cfg(test)]
            losses: Vec::new(),
            config,
        })
    }

    /// The stream's fixed metadata, to announce before any tile.
    pub fn meta(&self) -> &SpectrogramStreamMeta {
        &self.meta
    }

    /// Every sample lost because analysis fell more than the tap's ring
    /// behind the audio.
    pub fn lost_total(&self) -> u64 {
        self.reader.lost_total()
    }

    /// Analyses whatever audio is waiting and returns one tile, or `None` when
    /// no frame completed. Call again while it keeps returning tiles.
    ///
    /// A tile holds consecutive frames only. Where audio was lost the tile
    /// ends, and the next one resumes at the first frame past the gap.
    pub fn poll(&mut self) -> Option<SpectrogramTile> {
        let max_frames = self.config.max_frames_per_tile as usize;
        self.levels.clear();
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
            } else if index != start_frame + frames as u64 {
                // A gap: this frame belongs to the next tile, and its audio is
                // still in `frame` for that pass to analyse.
                self.ready_frame = Some(index);
                break;
            }
            self.analyze_current_frame();
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
            magnitudes: self.encode_levels(),
        };
        self.next_tile_seq += 1;
        Some(tile)
    }

    /// Fills the analysis frame until one is complete, returning its index on
    /// the stream's hop grid, or `None` when the reader has run dry.
    fn advance_to_next_frame(&mut self) -> Option<u64> {
        let size = self.config.fft_size;
        let hop = self.config.hop_size;

        loop {
            if self.staged.is_empty() {
                let read = self.reader.read(&mut self.scratch);
                if read.lost > 0 {
                    self.restart_after_loss(read.lost, read.count);
                }
                if read.count == 0 {
                    return None;
                }
                self.staged = 0..read.count;
            }

            if self.align_debt > 0 {
                let skip = self.align_debt.min(self.staged.len());
                self.staged.start += skip;
                self.align_debt -= skip;
                continue;
            }

            if self.filled == size {
                // The last frame is done with: keep its newest samples as the
                // start of the next, one hop on.
                self.frame.copy_within(hop.., 0);
                self.filled = size - hop;
            }
            let take = (size - self.filled).min(self.staged.len());
            let staged = self.staged.start..self.staged.start + take;
            self.frame[self.filled..self.filled + take].copy_from_slice(&self.scratch[staged]);
            self.filled += take;
            self.staged.start += take;
            if self.filled < size {
                continue;
            }

            // The frame ends where the unused samples begin, and the hop grid
            // is kept aligned, so its index follows from its position.
            let end = self.reader.position() - self.staged.len() as u64;
            let start = end - size as u64;
            debug_assert_eq!(start % hop as u64, 0, "frames stay on the hop grid");
            return Some(start / hop as u64);
        }
    }

    /// Drops the partial frame a loss interrupted, and lines the next frame
    /// up with the hop grid on the far side of the gap. `count` samples
    /// arrived after the loss in the same read.
    fn restart_after_loss(&mut self, lost: u64, count: usize) {
        let resume = self.reader.position() - count as u64;
        #[cfg(test)]
        self.losses.push((resume - lost, lost));
        #[cfg(not(test))]
        let _ = lost;

        let hop = self.config.hop_size as u64;
        self.filled = 0;
        self.align_debt = ((hop - resume % hop) % hop) as usize;
    }

    /// Windows the current frame, transforms it, and appends its levels.
    fn analyze_current_frame(&mut self) {
        for ((out, sample), weight) in self.windowed.iter_mut().zip(&self.frame).zip(&self.window) {
            *out = sample * weight;
        }
        self.fft.power(&self.windowed, &mut self.power);
        let floor = self.config.floor_db;
        let last_bin = self.power.len() - 1;
        for (bin, power) in self.power.iter().enumerate() {
            let scale = if bin == 0 || bin == last_bin {
                self.edge_power_scale
            } else {
                self.power_scale
            };
            let squared_amplitude = power * scale;
            let db = if squared_amplitude > self.floor_power {
                10.0 * squared_amplitude.log10()
            } else {
                floor
            };
            self.levels.push(db);
        }
    }

    /// Encodes the tile's levels as the stream declared.
    fn encode_levels(&self) -> SpectrogramMagnitudes {
        match self.config.encoding {
            SpectrogramEncoding::F32Json => SpectrogramMagnitudes::F32Json(
                self.levels
                    .iter()
                    .map(|db| (db * 10.0).round() / 10.0)
                    .collect(),
            ),
            SpectrogramEncoding::U8Base64 => {
                let floor = self.config.floor_db;
                let steps_per_db = 255.0 / (self.config.ceiling_db - floor);
                let bytes: Vec<u8> = self
                    .levels
                    .iter()
                    .map(|db| ((db - floor) * steps_per_db).round().clamp(0.0, 255.0) as u8)
                    .collect();
                SpectrogramMagnitudes::U8Base64(
                    base64::engine::general_purpose::STANDARD.encode(bytes),
                )
            }
        }
    }
}

#[cfg(test)]
mod tests;
