//! Input state for the SampleKit module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 2] = ["play", "key"];

pub struct SampleKitInputs {
    play: [f32; MAX_BLOCK],
    key: [f32; MAX_BLOCK],
    key_connected: bool,
}

impl SampleKitInputs {
    pub fn new() -> Self {
        Self {
            play: [0.0; MAX_BLOCK],
            key: [0.0; MAX_BLOCK],
            key_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "play" => {
                self.play.fill(value);
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
            0 => &mut self.play,
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
    pub fn play(&self, i: usize) -> f32 {
        self.play[i]
    }

    /// The key selecting a slot at frame `i`: the `key` input when connected,
    /// otherwise the `play` input's own value (so a bare play pulse can carry
    /// the key, e.g. a pulse of height 36 fires slot 36).
    #[inline]
    pub fn key(&self, i: usize) -> f32 {
        if self.key_connected {
            self.key[i]
        } else {
            self.play[i]
        }
    }
}

impl Default for SampleKitInputs {
    fn default() -> Self {
        Self::new()
    }
}
