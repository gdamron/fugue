//! Input state for the Lfo module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["sync", "rate"];

pub struct LfoInputs {
    sync: [f32; MAX_BLOCK],
    rate: [f32; MAX_BLOCK],
}

impl LfoInputs {
    pub fn new() -> Self {
        Self {
            sync: [0.0; MAX_BLOCK],
            rate: [0.0; MAX_BLOCK],
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "sync" => {
                self.sync.fill(value);
                Ok(())
            }
            "rate" => {
                self.rate.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.sync,
            _ => &mut self.rate,
        }
    }

    #[inline]
    pub fn sync(&self, i: usize) -> f32 {
        self.sync[i]
    }

    #[inline]
    pub fn rate(&self, i: usize) -> f32 {
        self.rate[i]
    }
}

impl Default for LfoInputs {
    fn default() -> Self {
        Self::new()
    }
}
