//! Output state for the CellSequencer module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 6] = ["frequency", "gate", "velocity", "step", "cell", "ended"];

pub struct CellSequencerOutputs {
    frequency: [f32; MAX_BLOCK],
    gate: [f32; MAX_BLOCK],
    velocity: [f32; MAX_BLOCK],
    step: [f32; MAX_BLOCK],
    cell: [f32; MAX_BLOCK],
    ended: [f32; MAX_BLOCK],
}

impl CellSequencerOutputs {
    pub fn new() -> Self {
        Self {
            frequency: [0.0; MAX_BLOCK],
            gate: [0.0; MAX_BLOCK],
            velocity: [1.0; MAX_BLOCK],
            step: [0.0; MAX_BLOCK],
            cell: [0.0; MAX_BLOCK],
            ended: [0.0; MAX_BLOCK],
        }
    }

    // Per-sample hot path: one plain argument per output port avoids building
    // a struct per sample, so keep the long signature.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn set(
        &mut self,
        i: usize,
        frequency: f32,
        gate: f32,
        velocity: f32,
        step: f32,
        cell: f32,
        ended: f32,
    ) {
        self.frequency[i] = frequency;
        self.gate[i] = gate;
        self.velocity[i] = velocity;
        self.step[i] = step;
        self.cell[i] = cell;
        self.ended[i] = ended;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.frequency,
            1 => &self.gate,
            2 => &self.velocity,
            3 => &self.step,
            4 => &self.cell,
            _ => &self.ended,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "frequency" => Ok(self.frequency[0]),
            "gate" => Ok(self.gate[0]),
            "velocity" => Ok(self.velocity[0]),
            "step" => Ok(self.step[0]),
            "cell" => Ok(self.cell[0]),
            "ended" => Ok(self.ended[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for CellSequencerOutputs {
    fn default() -> Self {
        Self::new()
    }
}
