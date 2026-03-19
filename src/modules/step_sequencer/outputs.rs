//! Output state for the StepSequencer module.

pub const OUTPUTS: [&str; 3] = ["frequency", "gate", "step"];

pub struct StepSequencerOutputs {
    frequency: f32,
    gate: f32,
    step: f32,
}

impl StepSequencerOutputs {
    pub fn new() -> Self {
        Self {
            frequency: 0.0,
            gate: 0.0,
            step: 0.0,
        }
    }

    pub fn set(&mut self, frequency: f32, gate: f32, step: f32) {
        self.frequency = frequency;
        self.gate = gate;
        self.step = step;
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "frequency" => Ok(self.frequency),
            "gate" => Ok(self.gate),
            "step" => Ok(self.step),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for StepSequencerOutputs {
    fn default() -> Self {
        Self::new()
    }
}
