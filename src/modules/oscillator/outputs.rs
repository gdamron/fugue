//! Output state for the Oscillator module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 1] = ["audio"];

pub struct OscillatorOutputs {
    audio: [f32; MAX_BLOCK],
}

impl OscillatorOutputs {
    pub fn new() -> Self {
        Self {
            audio: [0.0; MAX_BLOCK],
        }
    }

    /// Writes the generated sample for frame `i`.
    #[inline]
    pub fn set(&mut self, i: usize, value: f32) {
        self.audio[i] = value;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, _index: usize) -> &[f32] {
        &self.audio
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.audio[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for OscillatorOutputs {
    fn default() -> Self {
        Self::new()
    }
}
