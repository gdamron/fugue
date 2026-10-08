//! Input state for the Lfo module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["sync", "rate_mod"];

pub struct LfoInputs {
    sync: [f32; MAX_BLOCK],
    rate_mod: [f32; MAX_BLOCK],
}

impl LfoInputs {
    pub fn new() -> Self {
        Self {
            sync: [0.0; MAX_BLOCK],
            rate_mod: [0.0; MAX_BLOCK],
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "sync" => {
                self.sync.fill(value);
                Ok(())
            }
            "rate_mod" => {
                self.rate_mod.fill(value);
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
            _ => &mut self.rate_mod,
        }
    }

    #[inline]
    pub fn sync(&self, i: usize) -> f32 {
        self.sync[i]
    }

    #[inline]
    pub fn rate_mod(&self, i: usize) -> f32 {
        self.rate_mod[i]
    }
}

impl Default for LfoInputs {
    fn default() -> Self {
        Self::new()
    }
}
