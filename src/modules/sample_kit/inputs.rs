//! Input state for the SampleKit module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["trigger", "key"];

pub struct SampleKitInputs {
    trigger: [f32; MAX_BLOCK],
    key: [f32; MAX_BLOCK],
    key_connected: bool,
}

impl SampleKitInputs {
    pub fn new() -> Self {
        Self {
            trigger: [0.0; MAX_BLOCK],
            key: [0.0; MAX_BLOCK],
            key_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "trigger" => {
                self.trigger.fill(value);
                Ok(())
            }
            "key" => {
                self.key.fill(value);
                self.key_connected = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.trigger,
            _ => &mut self.key,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        if index == 1 {
            self.key_connected = connected;
        }
    }

    #[inline]
    pub fn trigger(&self, i: usize) -> f32 {
        self.trigger[i]
    }

    /// The key selecting a slot at frame `i`: the `key` input when connected,
    /// otherwise the trigger's own value (so a bare trigger signal can carry
    /// the key, e.g. a pulse of height 36 fires slot 36).
    #[inline]
    pub fn key(&self, i: usize) -> f32 {
        if self.key_connected {
            self.key[i]
        } else {
            self.trigger[i]
        }
    }
}

impl Default for SampleKitInputs {
    fn default() -> Self {
        Self::new()
    }
}
