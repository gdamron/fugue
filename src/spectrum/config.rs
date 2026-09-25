use crate::rpc::SpectrogramWindow;
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

/// Largest transform a stream may use: 2,049 bins, the most a spectrogram view
/// accepts. Larger sizes add no visible detail and cost memory on both ends.
pub const MAX_FFT_SIZE: usize = 4096;

/// Most frames a stream may ask a viewer to keep, matching the view's limit.
pub const MAX_HISTORY_FRAMES: u32 = 2048;

/// Largest tile a stream may send, matching the view's limit. Also bounds the
/// one allocation each tile makes.
pub const MAX_FRAMES_PER_TILE: u32 = 64;

impl SpectrumConfig {
    /// Checks the settings a stream cannot recover from, and that the stream
    /// fits within what a spectrogram view will accept.
    pub fn validate(&self) -> Result<(), String> {
        if self.fft_size < 32 || self.fft_size > MAX_FFT_SIZE || !self.fft_size.is_power_of_two() {
            return Err(format!(
                "fft_size must be a power of two from 32 to {MAX_FFT_SIZE}"
            ));
        }
        if self.hop_size == 0 || self.hop_size > self.fft_size {
            return Err("hop_size must be 1..=fft_size".to_string());
        }
        if !self.floor_db.is_finite() || !self.ceiling_db.is_finite() {
            return Err("floor_db and ceiling_db must be finite".to_string());
        }
        if self.floor_db >= self.ceiling_db {
            return Err("floor_db must be below ceiling_db".to_string());
        }
        if self.history_frames == 0 || self.history_frames > MAX_HISTORY_FRAMES {
            return Err(format!("history_frames must be 1..={MAX_HISTORY_FRAMES}"));
        }
        if self.max_frames_per_tile == 0 || self.max_frames_per_tile > MAX_FRAMES_PER_TILE {
            return Err(format!(
                "max_frames_per_tile must be 1..={MAX_FRAMES_PER_TILE}"
            ));
        }
        Ok(())
    }
}

/// Coefficients for one analysis window.
pub(super) fn window_coefficients(window: SpectrogramWindow, size: usize) -> Vec<f32> {
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
