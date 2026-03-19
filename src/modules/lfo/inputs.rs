//! Input state for the Lfo module.

pub const INPUTS: [&str; 2] = ["sync", "rate"];

pub struct LfoInputs {
    sync: f32,
    rate: f32,
}

impl LfoInputs {
    pub fn new() -> Self {
        Self {
            sync: 0.0,
            rate: 0.0,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "sync" => {
                self.sync = value;
                Ok(())
            }
            "rate" => {
                self.rate = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn sync(&self) -> f32 {
        self.sync
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }
}

impl Default for LfoInputs {
    fn default() -> Self {
        Self::new()
    }
}
