//! Output state for the AudioFileSink module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 3] = ["audio", "audio_left", "audio_right"];

pub struct AudioFileSinkOutputs {
    audio: [f32; MAX_BLOCK],
    audio_left: [f32; MAX_BLOCK],
    audio_right: [f32; MAX_BLOCK],
}

impl AudioFileSinkOutputs {
    pub fn new() -> Self {
        Self {
            audio: [0.0; MAX_BLOCK],
            audio_left: [0.0; MAX_BLOCK],
            audio_right: [0.0; MAX_BLOCK],
        }
    }

    #[inline]
    pub fn set(&mut self, i: usize, left: f32, right: f32) {
        self.audio[i] = (left + right) * 0.5;
        self.audio_left[i] = left;
        self.audio_right[i] = right;
    }

    #[inline]
    pub fn left_block(&self) -> &[f32] {
        &self.audio_left
    }

    #[inline]
    pub fn right_block(&self) -> &[f32] {
        &self.audio_right
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.audio,
            1 => &self.audio_left,
            _ => &self.audio_right,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.audio[0]),
            "audio_left" => Ok(self.audio_left[0]),
            "audio_right" => Ok(self.audio_right[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for AudioFileSinkOutputs {
    fn default() -> Self {
        Self::new()
    }
}
