//! Input state for the StepSequencer module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["gate", "reset"];

pub struct StepSequencerInputs {
    gate: [f32; MAX_BLOCK],
    reset: [f32; MAX_BLOCK],
}

impl StepSequencerInputs {
    pub fn new() -> Self {
        Self {
            gate: [0.0; MAX_BLOCK],
            reset: [0.0; MAX_BLOCK],
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => {
                self.gate.fill(value);
                Ok(())
            }
            "reset" => {
                self.reset.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.gate,
            _ => &mut self.reset,
        }
    }

    #[inline]
    pub fn gate(&self, i: usize) -> f32 {
        self.gate[i]
    }

    #[inline]
    pub fn reset(&self, i: usize) -> f32 {
        self.reset[i]
    }
}

impl Default for StepSequencerInputs {
    fn default() -> Self {
        Self::new()
    }
}
