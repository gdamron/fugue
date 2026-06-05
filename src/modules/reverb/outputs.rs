//! Output state for the Reverb module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 2] = ["left", "right"];

pub struct ReverbOutputs {
    left: [f32; MAX_BLOCK],
    right: [f32; MAX_BLOCK],
}

impl ReverbOutputs {
    pub fn new() -> Self {
        Self {
            left: [0.0; MAX_BLOCK],
            right: [0.0; MAX_BLOCK],
        }
    }

    #[inline]
    pub fn set(&mut self, i: usize, left: f32, right: f32) {
        self.left[i] = left;
        self.right[i] = right;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.left,
            _ => &self.right,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "left" => Ok(self.left[0]),
            "right" => Ok(self.right[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for ReverbOutputs {
    fn default() -> Self {
        Self::new()
    }
}
