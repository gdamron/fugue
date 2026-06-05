//! Output state for the MelodyGenerator module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 2] = ["frequency", "gate"];

pub struct MelodyOutputs {
    frequency: [f32; MAX_BLOCK],
    gate: [f32; MAX_BLOCK],
}

impl MelodyOutputs {
    pub fn new(initial_frequency: f32) -> Self {
        Self {
            frequency: [initial_frequency; MAX_BLOCK],
            gate: [0.0; MAX_BLOCK],
        }
    }

    #[inline]
    pub fn set(&mut self, i: usize, frequency: f32, gate: f32) {
        self.frequency[i] = frequency;
        self.gate[i] = gate;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.frequency,
            _ => &self.gate,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "frequency" => Ok(self.frequency[0]),
            "gate" => Ok(self.gate[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}
