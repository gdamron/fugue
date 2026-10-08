//! Output state for the Lfo module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 2] = ["bipolar", "unipolar"];

pub struct LfoOutputs {
    bipolar: [f32; MAX_BLOCK],
    unipolar: [f32; MAX_BLOCK],
}

impl LfoOutputs {
    pub fn new() -> Self {
        Self {
            bipolar: [0.0; MAX_BLOCK],
            unipolar: [0.5; MAX_BLOCK],
        }
    }

    /// Writes the bipolar and derived unipolar output for frame `i`.
    #[inline]
    pub fn set_bipolar(&mut self, i: usize, value: f32) {
        self.bipolar[i] = value;
        self.unipolar[i] = (value + 1.0) * 0.5;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.bipolar,
            _ => &self.unipolar,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "bipolar" => Ok(self.bipolar[0]),
            "unipolar" => Ok(self.unipolar[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for LfoOutputs {
    fn default() -> Self {
        Self::new()
    }
}
