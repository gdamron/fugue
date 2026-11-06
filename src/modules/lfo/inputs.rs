//! Input state for the Lfo module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 3] = ["sync", "rate_mod", "bpm"];
pub(super) const BPM: usize = 2;

pub struct LfoInputs {
    sync: [f32; MAX_BLOCK],
    rate_mod: [f32; MAX_BLOCK],
    bpm: [f32; MAX_BLOCK],
    /// Whether the `bpm` input is patched (or written), so it, not the
    /// `bpm` control, sets the tempo.
    bpm_connected: bool,
}

impl LfoInputs {
    pub fn new() -> Self {
        Self {
            sync: [0.0; MAX_BLOCK],
            rate_mod: [0.0; MAX_BLOCK],
            bpm: [0.0; MAX_BLOCK],
            bpm_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = INPUTS
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {}", port))?;
        self.block_mut(index).fill(value);
        if index == BPM {
            self.bpm_connected = true;
        }
        Ok(())
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.sync,
            1 => &mut self.rate_mod,
            _ => &mut self.bpm,
        }
    }

    pub fn set_connected(&mut self, index: usize, connected: bool) {
        if index == BPM {
            self.bpm_connected = connected;
        }
    }

    #[inline]
    pub fn sync(&self, i: usize) -> f32 {
        self.sync[i]
    }

    #[inline]
    pub fn rate_mod(&self, i: usize) -> f32 {
        self.rate_mod[i]
    }

    /// The patched tempo at the block's start, while `bpm` is patched.
    pub fn bpm(&self) -> Option<f32> {
        self.bpm_connected.then_some(self.bpm[0])
    }
}

impl Default for LfoInputs {
    fn default() -> Self {
        Self::new()
    }
}
