//! Output state for the Oscillator module.

pub const OUTPUTS: [&str; 1] = ["audio"];

pub struct OscillatorOutputs {
    audio: f32,
}

impl OscillatorOutputs {
    pub fn new() -> Self {
        Self { audio: 0.0 }
    }

    pub fn set_audio(&mut self, value: f32) {
        self.audio = value;
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.audio),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    /// Hot-path indexed getter. Index must match `OUTPUTS` order.
    #[inline]
    pub fn get_by_index(&self, index: usize) -> f32 {
        match index {
            0 => self.audio,
            _ => 0.0,
        }
    }
}

impl Default for OscillatorOutputs {
    fn default() -> Self {
        Self::new()
    }
}
