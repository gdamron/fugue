//! The chance module's input and output blocks.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["clock", "reset"];

pub const OUTPUTS: [&str; 9] = [
    "gate_1",
    "gate_2",
    "tick",
    "frequency_1",
    "frequency_2",
    "frequency_3",
    "value_1",
    "value_2",
    "value_3",
];

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
    blocks: [[f32; MAX_BLOCK]; 9],
}

impl Outputs {
    pub fn new() -> Self {
        Self {
            blocks: [[0.0; MAX_BLOCK]; 9],
        }
    }

    /// Writes frame `i` of every output: the three gates, then each
    /// channel's frequency and value.
    #[inline]
    pub fn set(&mut self, i: usize, gates: [bool; 3], frequencies: [f32; 3], values: [f32; 3]) {
        for (block, gate) in self.blocks[..3].iter_mut().zip(gates) {
            block[i] = if gate { 1.0 } else { 0.0 };
        }
        for (block, frequency) in self.blocks[3..6].iter_mut().zip(frequencies) {
            block[i] = frequency;
        }
        for (block, value) in self.blocks[6..].iter_mut().zip(values) {
            block[i] = value;
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
