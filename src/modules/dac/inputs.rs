//! Input state for the DacModule.

pub const INPUTS: [&str; 1] = ["audio"];

pub struct DacInputs {
    audio: f32,
}

impl DacInputs {
    pub fn new() -> Self {
        Self { audio: 0.0 }
    }

    pub fn reset(&mut self) {
        self.audio = 0.0;
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio += value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn audio(&self) -> f32 {
        self.audio
    }
}

impl Default for DacInputs {
    fn default() -> Self {
        Self::new()
    }
}
