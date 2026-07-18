//! Input buffers for `sample_slicer`.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["trigger", "slice"];

pub struct SampleSlicerInputs {
    trigger: [f32; MAX_BLOCK],
    slice: [f32; MAX_BLOCK],
}

impl SampleSlicerInputs {
    pub fn new(initial_slice: usize) -> Self {
        Self {
            trigger: [0.0; MAX_BLOCK],
            slice: [initial_slice as f32; MAX_BLOCK],
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "trigger" => self.trigger.fill(value),
            "slice" => self.slice.fill(value),
            _ => return Err(format!("Unknown input port: {}", port)),
        }
        Ok(())
    }

    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.trigger,
            _ => &mut self.slice,
        }
    }

    #[inline]
    pub fn trigger(&self, index: usize) -> f32 {
        self.trigger[index]
    }

    #[inline]
    pub fn slice(&self, index: usize) -> usize {
        self.slice[index].max(0.0).round() as usize
    }
}
