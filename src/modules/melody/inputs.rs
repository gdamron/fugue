//! Input state for the MelodyGenerator module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 1] = ["gate"];

pub struct MelodyInputs {
    gate: [f32; MAX_BLOCK],
}

impl MelodyInputs {
    pub fn new() -> Self {
        Self {
            gate: [0.0; MAX_BLOCK],
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => {
                self.gate.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.gate
    }

    #[inline]
    pub fn gate(&self, i: usize) -> f32 {
        self.gate[i]
    }
}

impl Default for MelodyInputs {
    fn default() -> Self {
        Self::new()
    }
}
