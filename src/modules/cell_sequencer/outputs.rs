//! Output state for the CellSequencer module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 4] = ["frequency", "gate", "step", "sequence"];

pub struct CellSequencerOutputs {
    frequency: [f32; MAX_BLOCK],
    gate: [f32; MAX_BLOCK],
    step: [f32; MAX_BLOCK],
    sequence: [f32; MAX_BLOCK],
}

impl CellSequencerOutputs {
    pub fn new() -> Self {
        Self {
            frequency: [0.0; MAX_BLOCK],
            gate: [0.0; MAX_BLOCK],
            step: [0.0; MAX_BLOCK],
            sequence: [0.0; MAX_BLOCK],
        }
    }

    #[inline]
    pub fn set(&mut self, i: usize, frequency: f32, gate: f32, step: f32, sequence: f32) {
        self.frequency[i] = frequency;
        self.gate[i] = gate;
        self.step[i] = step;
        self.sequence[i] = sequence;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.frequency,
            1 => &self.gate,
            2 => &self.step,
            _ => &self.sequence,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "frequency" => Ok(self.frequency[0]),
            "gate" => Ok(self.gate[0]),
            "step" => Ok(self.step[0]),
            "sequence" => Ok(self.sequence[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for CellSequencerOutputs {
    fn default() -> Self {
        Self::new()
    }
}
