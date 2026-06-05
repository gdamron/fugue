//! Input state for the DacModule.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 3] = ["audio", "audio_left", "audio_right"];

pub struct DacInputs {
    audio: [f32; MAX_BLOCK],
    audio_left: [f32; MAX_BLOCK],
    audio_right: [f32; MAX_BLOCK],
}

impl DacInputs {
    pub fn new() -> Self {
        Self {
            audio: [0.0; MAX_BLOCK],
            audio_left: [0.0; MAX_BLOCK],
            audio_right: [0.0; MAX_BLOCK],
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    /// In the graph, multiple sources into a port are summed by the scheduler.
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio.fill(value);
                Ok(())
            }
            "audio_left" => {
                self.audio_left.fill(value);
                Ok(())
            }
            "audio_right" => {
                self.audio_right.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.audio,
            1 => &mut self.audio_left,
            _ => &mut self.audio_right,
        }
    }

    /// Effective left input at frame `i`: the mono `audio` port mixed into left.
    #[inline]
    pub fn audio_left(&self, i: usize) -> f32 {
        self.audio_left[i] + self.audio[i]
    }

    /// Effective right input at frame `i`: the mono `audio` port mixed into right.
    #[inline]
    pub fn audio_right(&self, i: usize) -> f32 {
        self.audio_right[i] + self.audio[i]
    }
}

impl Default for DacInputs {
    fn default() -> Self {
        Self::new()
    }
}
