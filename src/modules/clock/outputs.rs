//! Output state for the Clock module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 5] = ["gate", "gate_d4", "gate_d2", "gate_x2", "gate_x4"];

pub struct ClockOutputs {
    gate: [f32; MAX_BLOCK],
    gate_d4: [f32; MAX_BLOCK],
    gate_d2: [f32; MAX_BLOCK],
    gate_x2: [f32; MAX_BLOCK],
    gate_x4: [f32; MAX_BLOCK],
}

impl ClockOutputs {
    pub fn new() -> Self {
        Self {
            gate: [0.0; MAX_BLOCK],
            gate_d4: [0.0; MAX_BLOCK],
            gate_d2: [0.0; MAX_BLOCK],
            gate_x2: [0.0; MAX_BLOCK],
            gate_x4: [0.0; MAX_BLOCK],
        }
    }

    /// Writes all five gate outputs for frame `i`.
    #[inline]
    pub fn set_all(
        &mut self,
        i: usize,
        gate: f32,
        gate_d4: f32,
        gate_d2: f32,
        gate_x2: f32,
        gate_x4: f32,
    ) {
        self.gate[i] = gate;
        self.gate_d4[i] = gate_d4;
        self.gate_d2[i] = gate_d2;
        self.gate_x2[i] = gate_x2;
        self.gate_x4[i] = gate_x4;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.gate,
            1 => &self.gate_d4,
            2 => &self.gate_d2,
            3 => &self.gate_x2,
            _ => &self.gate_x4,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "gate" => Ok(self.gate[0]),
            "gate_d4" => Ok(self.gate_d4[0]),
            "gate_d2" => Ok(self.gate_d2[0]),
            "gate_x2" => Ok(self.gate_x2[0]),
            "gate_x4" => Ok(self.gate_x4[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for ClockOutputs {
    fn default() -> Self {
        Self::new()
    }
}
