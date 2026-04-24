//! Input state for the MelodyGenerator module.

pub const INPUTS: [&str; 1] = ["gate"];

pub struct MelodyInputs {
    gate: f32,
}

impl MelodyInputs {
    pub fn new() -> Self {
        Self { gate: 0.0 }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => {
                self.gate = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Hot-path indexed setter. Index must match `INPUTS` order.
    #[inline]
    pub fn set_by_index(&mut self, index: usize, value: f32) {
        if index == 0 {
            self.gate = value;
        }
    }

    pub fn gate(&self) -> f32 {
        self.gate
    }
}

impl Default for MelodyInputs {
    fn default() -> Self {
        Self::new()
    }
}
