//! Output state for the Adsr module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 1] = ["envelope"];

pub struct AdsrOutputs {
    envelope: [f32; MAX_BLOCK],
}

impl AdsrOutputs {
    pub fn new() -> Self {
        Self {
            envelope: [0.0; MAX_BLOCK],
        }
    }

    /// Writes the (clamped) envelope value for frame `i`.
    #[inline]
    pub fn set(&mut self, i: usize, envelope: f32) {
        self.envelope[i] = envelope.clamp(0.0, 1.0);
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, _index: usize) -> &[f32] {
        &self.envelope
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "envelope" => Ok(self.envelope[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for AdsrOutputs {
    fn default() -> Self {
        Self::new()
    }
}
