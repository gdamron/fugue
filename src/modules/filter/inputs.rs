//! Input state for the Filter module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 4] = ["audio", "cutoff", "cutoff_cv", "resonance"];

pub struct FilterInputs {
    audio: [f32; MAX_BLOCK],
    cutoff: [f32; MAX_BLOCK],
    cutoff_cv: [f32; MAX_BLOCK],
    resonance: [f32; MAX_BLOCK],
    cutoff_connected: bool,
    resonance_connected: bool,
}

impl FilterInputs {
    pub fn new() -> Self {
        Self {
            audio: [0.0; MAX_BLOCK],
            cutoff: [0.0; MAX_BLOCK],
            cutoff_cv: [0.0; MAX_BLOCK],
            resonance: [0.0; MAX_BLOCK],
            cutoff_connected: false,
            resonance_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio.fill(value);
                Ok(())
            }
            "cutoff" => {
                self.cutoff.fill(value.clamp(20.0, 20000.0));
                self.cutoff_connected = true;
                Ok(())
            }
            "cutoff_cv" => {
                self.cutoff_cv.fill(value);
                Ok(())
            }
            "resonance" => {
                self.resonance.fill(value.clamp(0.0, 1.0));
                self.resonance_connected = true;
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
            1 => &mut self.cutoff,
            2 => &mut self.cutoff_cv,
            _ => &mut self.resonance,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        match index {
            1 => self.cutoff_connected = connected,
            3 => self.resonance_connected = connected,
            _ => {}
        }
    }

    #[inline]
    pub fn audio(&self, i: usize) -> f32 {
        self.audio[i]
    }

    #[inline]
    pub fn cutoff(&self, i: usize, control: f32) -> f32 {
        if self.cutoff_connected {
            self.cutoff[i].clamp(20.0, 20000.0)
        } else {
            control
        }
    }

    #[inline]
    pub fn cutoff_cv(&self, i: usize) -> f32 {
        self.cutoff_cv[i]
    }

    #[inline]
    pub fn resonance(&self, i: usize, control: f32) -> f32 {
        if self.resonance_connected {
            self.resonance[i].clamp(0.0, 1.0)
        } else {
            control
        }
    }
}

impl Default for FilterInputs {
    fn default() -> Self {
        Self::new()
    }
}
