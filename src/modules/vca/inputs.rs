//! Input state for the Vca module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["audio", "cv"];

pub struct VcaInputs {
    audio: [f32; MAX_BLOCK],
    cv: [f32; MAX_BLOCK],
    cv_connected: bool,
}

impl VcaInputs {
    pub fn new() -> Self {
        Self {
            audio: [0.0; MAX_BLOCK],
            cv: [1.0; MAX_BLOCK],
            cv_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio.fill(value);
                Ok(())
            }
            "cv" => {
                self.cv.fill(value.clamp(0.0, 1.0));
                self.cv_connected = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.audio,
            _ => &mut self.cv,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        if index == 1 {
            self.cv_connected = connected;
        }
    }

    #[inline]
    pub fn audio(&self, i: usize) -> f32 {
        self.audio[i]
    }

    /// Effective CV at frame `i`: the connected signal (clamped) or the control default.
    #[inline]
    pub fn cv(&self, i: usize, control: f32) -> f32 {
        if self.cv_connected {
            self.cv[i].clamp(0.0, 1.0)
        } else {
            control
        }
    }
}

impl Default for VcaInputs {
    fn default() -> Self {
        Self::new()
    }
}
