//! Output state for the CellSequencer module.

pub const OUTPUTS: [&str; 4] = ["frequency", "gate", "step", "sequence"];

pub struct CellSequencerOutputs {
    frequency: f32,
    gate: f32,
    step: f32,
    sequence: f32,
}

impl CellSequencerOutputs {
    pub fn new() -> Self {
        Self {
            frequency: 0.0,
            gate: 0.0,
            step: 0.0,
            sequence: 0.0,
        }
    }

    pub fn set(&mut self, frequency: f32, gate: f32, step: f32, sequence: f32) {
        self.frequency = frequency;
        self.gate = gate;
        self.step = step;
        self.sequence = sequence;
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "frequency" => Ok(self.frequency),
            "gate" => Ok(self.gate),
            "step" => Ok(self.step),
            "sequence" => Ok(self.sequence),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for CellSequencerOutputs {
    fn default() -> Self {
        Self::new()
    }
}
