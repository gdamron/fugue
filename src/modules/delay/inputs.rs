//! Input state for the delay.

use crate::MAX_BLOCK;

pub(super) const INPUTS: [&str; 3] = ["audio", "time_mod", "bpm"];
const BPM: usize = 2;

pub(super) struct DelayInputs {
    pub(super) audio: [f32; MAX_BLOCK],
    pub(super) time_mod: [f32; MAX_BLOCK],
    pub(super) bpm: [f32; MAX_BLOCK],
    /// Whether the `bpm` input is patched (or written), so it, not the
    /// `bpm` control, sets the tempo.
    pub(super) bpm_connected: bool,
}

impl DelayInputs {
    pub(super) fn new() -> Self {
        Self {
            audio: [0.0; MAX_BLOCK],
            time_mod: [0.0; MAX_BLOCK],
            bpm: [0.0; MAX_BLOCK],
            bpm_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub(super) fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = INPUTS
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {port}"))?;
        self.block_mut(index).fill(value);
        if index == BPM {
            self.bpm_connected = true;
        }
        Ok(())
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub(super) fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.audio,
            1 => &mut self.time_mod,
            _ => &mut self.bpm,
        }
    }

    pub(super) fn set_connected(&mut self, index: usize, connected: bool) {
        if index == BPM {
            self.bpm_connected = connected;
        }
    }
}
