//! The chance module's input and output blocks.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["clock", "reset"];

pub const OUTPUTS: [&str; 3] = ["gate_1", "gate_2", "tick"];

pub struct Inputs {
    blocks: [[f32; MAX_BLOCK]; 2],
}

impl Inputs {
    pub fn new() -> Self {
        Self {
            blocks: [[0.0; MAX_BLOCK]; 2],
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = INPUTS
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {port}"))?;
        self.blocks[index].fill(value);
        Ok(())
    }

    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.blocks[index.min(1)]
    }

    #[inline]
    pub fn clock(&self, i: usize) -> f32 {
        self.blocks[0][i]
    }

    #[inline]
    pub fn reset(&self, i: usize) -> f32 {
        self.blocks[1][i]
    }
}

pub struct Outputs {
    blocks: [[f32; MAX_BLOCK]; 3],
}

impl Outputs {
    pub fn new() -> Self {
        Self {
            blocks: [[0.0; MAX_BLOCK]; 3],
        }
    }

    /// Writes frame `i` of the three gates.
    #[inline]
    pub fn set(&mut self, i: usize, gates: [bool; 3]) {
        for (block, gate) in self.blocks.iter_mut().zip(gates) {
            block[i] = if gate { 1.0 } else { 0.0 };
        }
    }

    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        &self.blocks[index.min(OUTPUTS.len() - 1)]
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        let index = OUTPUTS
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown output port: {port}"))?;
        Ok(self.blocks[index][0])
    }
}
