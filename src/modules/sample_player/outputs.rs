//! Output state for the SamplePlayer module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 4] = [
    "audio_left",
    "audio_right",
    "sample_start_gate",
    "sample_end_gate",
];

pub struct SamplePlayerOutputs {
    audio_left: [f32; MAX_BLOCK],
    audio_right: [f32; MAX_BLOCK],
    sample_start_gate: [f32; MAX_BLOCK],
    sample_end_gate: [f32; MAX_BLOCK],
}

impl SamplePlayerOutputs {
    pub fn new() -> Self {
        Self {
            audio_left: [0.0; MAX_BLOCK],
            audio_right: [0.0; MAX_BLOCK],
            sample_start_gate: [0.0; MAX_BLOCK],
            sample_end_gate: [0.0; MAX_BLOCK],
        }
    }

    #[inline]
    pub fn set(
        &mut self,
        i: usize,
        audio_left: f32,
        audio_right: f32,
        start_gate: f32,
        end_gate: f32,
    ) {
        self.audio_left[i] = audio_left;
        self.audio_right[i] = audio_right;
        self.sample_start_gate[i] = start_gate;
        self.sample_end_gate[i] = end_gate;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.audio_left,
            1 => &self.audio_right,
            2 => &self.sample_start_gate,
            _ => &self.sample_end_gate,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio_left" => Ok(self.audio_left[0]),
            "audio_right" => Ok(self.audio_right[0]),
            "sample_start_gate" => Ok(self.sample_start_gate[0]),
            "sample_end_gate" => Ok(self.sample_end_gate[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for SamplePlayerOutputs {
    fn default() -> Self {
        Self::new()
    }
}
