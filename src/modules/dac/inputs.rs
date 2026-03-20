//! Input state for the DacModule.

pub const INPUTS: [&str; 3] = ["audio", "audio_left", "audio_right"];

pub struct DacInputs {
    audio: f32,
    audio_left: f32,
    audio_right: f32,
}

impl DacInputs {
    pub fn new() -> Self {
        Self {
            audio: 0.0,
            audio_left: 0.0,
            audio_right: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.audio = 0.0;
        self.audio_left = 0.0;
        self.audio_right = 0.0;
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio += value;
                Ok(())
            }
            "audio_left" => {
                self.audio_left += value;
                Ok(())
            }
            "audio_right" => {
                self.audio_right += value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn audio_left(&self) -> f32 {
        self.audio_left + self.audio
    }

    pub fn audio_right(&self) -> f32 {
        self.audio_right + self.audio
    }
}

impl Default for DacInputs {
    fn default() -> Self {
        Self::new()
    }
}
