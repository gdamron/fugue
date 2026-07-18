//! Output buffers for `sample_slicer`.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 4] = [
    "audio_left",
    "audio_right",
    "slice_start_gate",
    "slice_end_gate",
];

pub struct SampleSlicerOutputs {
    audio_left: [f32; MAX_BLOCK],
    audio_right: [f32; MAX_BLOCK],
    slice_start_gate: [f32; MAX_BLOCK],
    slice_end_gate: [f32; MAX_BLOCK],
}

impl SampleSlicerOutputs {
    pub fn new() -> Self {
        Self {
            audio_left: [0.0; MAX_BLOCK],
            audio_right: [0.0; MAX_BLOCK],
            slice_start_gate: [0.0; MAX_BLOCK],
            slice_end_gate: [0.0; MAX_BLOCK],
        }
    }

    #[inline]
    pub fn set(&mut self, index: usize, left: f32, right: f32, start_gate: f32, end_gate: f32) {
        self.audio_left[index] = left;
        self.audio_right[index] = right;
        self.slice_start_gate[index] = start_gate;
        self.slice_end_gate[index] = end_gate;
    }

    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.audio_left,
            1 => &self.audio_right,
            2 => &self.slice_start_gate,
            _ => &self.slice_end_gate,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio_left" => Ok(self.audio_left[0]),
            "audio_right" => Ok(self.audio_right[0]),
            "slice_start_gate" => Ok(self.slice_start_gate[0]),
            "slice_end_gate" => Ok(self.slice_end_gate[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}
