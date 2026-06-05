//! Input state for the Reverb module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["left", "right"];

pub struct ReverbInputs {
    left: [f32; MAX_BLOCK],
    right: [f32; MAX_BLOCK],
    right_connected: bool,
}

impl ReverbInputs {
    pub fn new() -> Self {
        Self {
            left: [0.0; MAX_BLOCK],
            right: [0.0; MAX_BLOCK],
            right_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "left" => {
                self.left.fill(value);
                Ok(())
            }
            "right" => {
                self.right.fill(value);
                self.right_connected = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.left,
            _ => &mut self.right,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        if index == 1 {
            self.right_connected = connected;
        }
    }

    #[inline]
    pub fn left(&self, i: usize) -> f32 {
        self.left[i]
    }

    #[inline]
    pub fn right(&self, i: usize) -> f32 {
        self.right[i]
    }

    /// Returns true if the right input is fed by an upstream connection.
    #[inline]
    pub fn right_active(&self) -> bool {
        self.right_connected
    }
}

impl Default for ReverbInputs {
    fn default() -> Self {
        Self::new()
    }
}
