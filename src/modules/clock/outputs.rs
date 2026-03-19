//! Output state for the Clock module.

pub const OUTPUTS: [&str; 1] = ["gate"];

pub struct ClockOutputs {
    gate: f32,
}

impl ClockOutputs {
    pub fn new() -> Self {
        Self { gate: 0.0 }
    }

    pub fn set_gate(&mut self, value: f32) {
        self.gate = value;
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "gate" => Ok(self.gate),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for ClockOutputs {
    fn default() -> Self {
        Self::new()
    }
}
