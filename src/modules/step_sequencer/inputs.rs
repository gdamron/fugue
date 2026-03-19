//! Input state for the StepSequencer module.

pub const INPUTS: [&str; 2] = ["gate", "reset"];

pub struct StepSequencerInputs {
    gate: f32,
    reset: f32,
}

impl StepSequencerInputs {
    pub fn new() -> Self {
        Self {
            gate: 0.0,
            reset: 0.0,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => {
                self.gate = value;
                Ok(())
            }
            "reset" => {
                self.reset = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn gate(&self) -> f32 {
        self.gate
    }

    pub fn reset(&self) -> f32 {
        self.reset
    }
}

impl Default for StepSequencerInputs {
    fn default() -> Self {
        Self::new()
    }
}
