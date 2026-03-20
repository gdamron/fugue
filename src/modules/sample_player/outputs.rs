//! Output state for the SamplePlayer module.

pub const OUTPUTS: [&str; 4] = [
    "audio_left",
    "audio_right",
    "sample_start_gate",
    "sample_end_gate",
];

pub struct SamplePlayerOutputs {
    audio_left: f32,
    audio_right: f32,
    sample_start_gate: f32,
    sample_end_gate: f32,
}

impl SamplePlayerOutputs {
    pub fn new() -> Self {
        Self {
            audio_left: 0.0,
            audio_right: 0.0,
            sample_start_gate: 0.0,
            sample_end_gate: 0.0,
        }
    }

    pub fn set(&mut self, audio_left: f32, audio_right: f32, start_gate: f32, end_gate: f32) {
        self.audio_left = audio_left;
        self.audio_right = audio_right;
        self.sample_start_gate = start_gate;
        self.sample_end_gate = end_gate;
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio_left" => Ok(self.audio_left),
            "audio_right" => Ok(self.audio_right),
            "sample_start_gate" => Ok(self.sample_start_gate),
            "sample_end_gate" => Ok(self.sample_end_gate),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for SamplePlayerOutputs {
    fn default() -> Self {
        Self::new()
    }
}
