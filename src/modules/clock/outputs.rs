//! Output state for the Clock module.

pub const OUTPUTS: [&str; 5] = ["gate", "gate_d4", "gate_d2", "gate_x2", "gate_x4"];

pub struct ClockOutputs {
    gate: f32,
    gate_d4: f32,
    gate_d2: f32,
    gate_x2: f32,
    gate_x4: f32,
}

impl ClockOutputs {
    pub fn new() -> Self {
        Self {
            gate: 0.0,
            gate_d4: 0.0,
            gate_d2: 0.0,
            gate_x2: 0.0,
            gate_x4: 0.0,
        }
    }

    pub fn set_all(&mut self, gate: f32, gate_d4: f32, gate_d2: f32, gate_x2: f32, gate_x4: f32) {
        self.gate = gate;
        self.gate_d4 = gate_d4;
        self.gate_d2 = gate_d2;
        self.gate_x2 = gate_x2;
        self.gate_x4 = gate_x4;
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "gate" => Ok(self.gate),
            "gate_d4" => Ok(self.gate_d4),
            "gate_d2" => Ok(self.gate_d2),
            "gate_x2" => Ok(self.gate_x2),
            "gate_x4" => Ok(self.gate_x4),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for ClockOutputs {
    fn default() -> Self {
        Self::new()
    }
}
