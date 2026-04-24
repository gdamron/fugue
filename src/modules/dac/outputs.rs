//! Output state for the DacModule.

pub const OUTPUTS: [&str; 3] = ["audio", "audio_left", "audio_right"];

pub struct DacOutputs {
    audio: f32,
    audio_left: f32,
    audio_right: f32,
}

impl DacOutputs {
    pub fn new() -> Self {
        Self {
            audio: 0.0,
            audio_left: 0.0,
            audio_right: 0.0,
        }
    }

    pub fn set(&mut self, left: f32, right: f32) {
        self.audio = (left + right) * 0.5;
        self.audio_left = left;
        self.audio_right = right;
    }

    pub fn audio_left(&self) -> f32 {
        self.audio_left
    }

    pub fn audio_right(&self) -> f32 {
        self.audio_right
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.audio),
            "audio_left" => Ok(self.audio_left),
            "audio_right" => Ok(self.audio_right),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    /// Hot-path indexed getter. Index must match `OUTPUTS` order.
    #[inline]
    pub fn get_by_index(&self, index: usize) -> f32 {
        match index {
            0 => self.audio,
            1 => self.audio_left,
            2 => self.audio_right,
            _ => 0.0,
        }
    }
}

impl Default for DacOutputs {
    fn default() -> Self {
        Self::new()
    }
}
