//! Output state for the MelodyGenerator module.

pub const OUTPUTS: [&str; 2] = ["frequency", "gate"];

pub struct MelodyOutputs {
    frequency: f32,
    gate: f32,
}

impl MelodyOutputs {
    pub fn new(initial_frequency: f32) -> Self {
        Self {
            frequency: initial_frequency,
            gate: 0.0,
        }
    }

    pub fn set(&mut self, frequency: f32, gate: f32) {
        self.frequency = frequency;
        self.gate = gate;
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "frequency" => Ok(self.frequency),
            "gate" => Ok(self.gate),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}
