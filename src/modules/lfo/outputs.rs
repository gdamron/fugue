//! Output state for the Lfo module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 2] = ["out", "out_uni"];

pub struct LfoOutputs {
    out: [f32; MAX_BLOCK],
    out_uni: [f32; MAX_BLOCK],
}

impl LfoOutputs {
    pub fn new() -> Self {
        Self {
            out: [0.0; MAX_BLOCK],
            out_uni: [0.5; MAX_BLOCK],
        }
    }

    /// Writes the bipolar and derived unipolar output for frame `i`.
    #[inline]
    pub fn set_bipolar(&mut self, i: usize, value: f32) {
        self.out[i] = value;
        self.out_uni[i] = (value + 1.0) * 0.5;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.out,
            _ => &self.out_uni,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "out" => Ok(self.out[0]),
            "out_uni" => Ok(self.out_uni[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for LfoOutputs {
    fn default() -> Self {
        Self::new()
    }
}
