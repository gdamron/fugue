//! Output state for the SampleInstrument module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 2] = ["audio_left", "audio_right"];

pub struct SampleInstrumentOutputs {
    audio_left: [f32; MAX_BLOCK],
    audio_right: [f32; MAX_BLOCK],
}

impl SampleInstrumentOutputs {
    pub fn new() -> Self {
        Self {
            audio_left: [0.0; MAX_BLOCK],
            audio_right: [0.0; MAX_BLOCK],
        }
    }

    #[inline]
    pub fn set(&mut self, i: usize, left: f32, right: f32) {
        self.audio_left[i] = left;
        self.audio_right[i] = right;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.audio_left,
            _ => &self.audio_right,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio_left" => Ok(self.audio_left[0]),
            "audio_right" => Ok(self.audio_right[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for SampleInstrumentOutputs {
    fn default() -> Self {
        Self::new()
    }
}
