//! Output buffers for `sample_slicer`.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 4] = ["audio_left", "audio_right", "start", "end"];

pub struct SampleSlicerOutputs {
    audio_left: [f32; MAX_BLOCK],
    audio_right: [f32; MAX_BLOCK],
    start: [f32; MAX_BLOCK],
    end: [f32; MAX_BLOCK],
}

impl SampleSlicerOutputs {
    pub fn new() -> Self {
        Self {
            audio_left: [0.0; MAX_BLOCK],
            audio_right: [0.0; MAX_BLOCK],
            start: [0.0; MAX_BLOCK],
            end: [0.0; MAX_BLOCK],
        }
    }

    #[inline]
    pub fn set(&mut self, index: usize, left: f32, right: f32, start_gate: f32, end_gate: f32) {
        self.audio_left[index] = left;
        self.audio_right[index] = right;
        self.start[index] = start_gate;
        self.end[index] = end_gate;
    }

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
