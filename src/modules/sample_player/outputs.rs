//! Output state for the SamplePlayer module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 4] = ["audio_left", "audio_right", "start", "end"];

pub struct SamplePlayerOutputs {
    audio_left: [f32; MAX_BLOCK],
    audio_right: [f32; MAX_BLOCK],
    start: [f32; MAX_BLOCK],
    end: [f32; MAX_BLOCK],
}

impl SamplePlayerOutputs {
    pub fn new() -> Self {
        Self {
            audio_left: [0.0; MAX_BLOCK],
            audio_right: [0.0; MAX_BLOCK],
            start: [0.0; MAX_BLOCK],
            end: [0.0; MAX_BLOCK],
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
        self.start[i] = start_gate;
        self.end[i] = end_gate;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.audio_left,
            1 => &self.audio_right,
            2 => &self.start,
            _ => &self.end,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio_left" => Ok(self.audio_left[0]),
            "audio_right" => Ok(self.audio_right[0]),
            "start" => Ok(self.start[0]),
            "end" => Ok(self.end[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for SamplePlayerOutputs {
    fn default() -> Self {
        Self::new()
    }
}
