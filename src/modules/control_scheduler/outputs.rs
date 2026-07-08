//! Output state for the ControlScheduler module.

use crate::MAX_BLOCK;

pub const OUTPUTS: [&str; 1] = ["step"];

pub struct ControlSchedulerOutputs {
    step: [f32; MAX_BLOCK],
}

impl ControlSchedulerOutputs {
    pub fn new() -> Self {
        Self {
            step: [-1.0; MAX_BLOCK],
        }
    }

    #[inline]
    pub fn set(&mut self, i: usize, step: f32) {
        self.step[i] = step;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, _index: usize) -> &[f32] {
        &self.step
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "step" => Ok(self.step[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for ControlSchedulerOutputs {
    fn default() -> Self {
        Self::new()
    }
}
