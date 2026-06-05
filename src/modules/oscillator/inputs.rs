//! Input state for the Oscillator module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 3] = ["frequency", "fm", "am"];

pub struct OscillatorInputs {
    frequency: [f32; MAX_BLOCK],
    fm: [f32; MAX_BLOCK],
    am: [f32; MAX_BLOCK],
    frequency_connected: bool,
}

impl OscillatorInputs {
    pub fn new() -> Self {
        Self {
            frequency: [0.0; MAX_BLOCK],
            fm: [0.0; MAX_BLOCK],
            am: [0.0; MAX_BLOCK],
            frequency_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "frequency" => {
                self.frequency.fill(value);
                self.frequency_connected = true;
                Ok(())
            }
            "fm" => {
                self.fm.fill(value);
                Ok(())
            }
            "am" => {
                self.am.fill(value);
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.frequency,
            1 => &mut self.fm,
            _ => &mut self.am,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        if index == 0 {
            self.frequency_connected = connected;
        }
    }

    /// Effective frequency at frame `i`: the connected signal or the control default.
    #[inline]
    pub fn frequency(&self, i: usize, control: f32) -> f32 {
        if self.frequency_connected {
            self.frequency[i]
        } else {
            control
        }
    }

    #[inline]
    pub fn fm(&self, i: usize) -> f32 {
        self.fm[i]
    }

    #[inline]
    pub fn am(&self, i: usize) -> f32 {
        self.am[i]
    }
}

impl Default for OscillatorInputs {
    fn default() -> Self {
        Self::new()
    }
}
