//! Input state for the Mixer module.

use super::MAX_CHANNELS;
use crate::modules::indexed_names::IndexedNames;
use crate::MAX_BLOCK;

pub(super) static AUDIO_NAMES: IndexedNames = IndexedNames::new("audio", MAX_CHANNELS);
pub(super) static LEVEL_NAMES: IndexedNames = IndexedNames::new("level", MAX_CHANNELS);
pub(super) static PAN_NAMES: IndexedNames = IndexedNames::new("pan", MAX_CHANNELS);

pub struct MixerInputs {
    names: Vec<&'static str>,
    channels: usize,
    audio: Vec<[f32; MAX_BLOCK]>,
    level_cvs: Vec<[f32; MAX_BLOCK]>,
    pan_mods: Vec<[f32; MAX_BLOCK]>,
    master_cv: [f32; MAX_BLOCK],
    level_cv_connected: Vec<bool>,
    pan_mod_connected: Vec<bool>,
    master_cv_connected: bool,
}

impl MixerInputs {
    pub fn new(channels: usize) -> Self {
        let mut names = Vec::with_capacity(channels * 3 + 1);
        names.extend(AUDIO_NAMES.first(channels));
        names.extend(LEVEL_NAMES.first(channels));
        names.extend(PAN_NAMES.first(channels));
        names.push("master");

        Self {
            names,
            channels,
            audio: vec![[0.0; MAX_BLOCK]; channels],
            level_cvs: vec![[1.0; MAX_BLOCK]; channels],
            pan_mods: vec![[0.0; MAX_BLOCK]; channels],
            master_cv: [1.0; MAX_BLOCK],
            level_cv_connected: vec![false; channels],
            pan_mod_connected: vec![false; channels],
            master_cv_connected: false,
        }
    }

    pub fn names(&self) -> &[&str] {
        &self.names
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, channels: usize, port: &str, value: f32) -> Result<(), String> {
        if let Some(idx) = AUDIO_NAMES.index_of(port, channels) {
            self.audio[idx].fill(value);
            return Ok(());
        }

        if let Some(idx) = LEVEL_NAMES.index_of(port, channels) {
            self.level_cvs[idx].fill(value.clamp(0.0, 2.0));
            self.level_cv_connected[idx] = true;
            return Ok(());
        }

        if let Some(idx) = PAN_NAMES.index_of(port, channels) {
            self.pan_mods[idx].fill(value.clamp(-1.0, 1.0));
            self.pan_mod_connected[idx] = true;
            return Ok(());
        }

        if port == "master" {
            self.master_cv.fill(value.clamp(0.0, 2.0));
            self.master_cv_connected = true;
            return Ok(());
        }

        Err(format!("Unknown input port: {}", port))
    }

    /// Mutable block buffer for the indexed input port. Port layout for a
    /// mixer with N channels:
    ///   `[0, N)` → audio inputs (audio.0..audio.N-1)
    ///   `[N, 2N)` → level CVs (level.0..level.N-1)
    ///   `[2N, 3N)` → pan mods (pan.0..pan.N-1)
    ///   `3N` → master CV
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        let n = self.channels;
        if index < n {
            &mut self.audio[index]
        } else if index < 2 * n {
            &mut self.level_cvs[index - n]
        } else if index < 3 * n {
            &mut self.pan_mods[index - 2 * n]
        } else {
            &mut self.master_cv
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        let n = self.channels;
        if index < n {
            // audio inputs do not arbitrate against a control default
        } else if index < 2 * n {
            self.level_cv_connected[index - n] = connected;
        } else if index < 3 * n {
            self.pan_mod_connected[index - 2 * n] = connected;
        } else if index == 3 * n {
            self.master_cv_connected = connected;
        }
    }

    #[inline]
    pub fn audio(&self, channel: usize, i: usize) -> f32 {
        self.audio[channel][i]
    }

    #[inline]
    pub fn level_cv(&self, channel: usize, i: usize) -> f32 {
        if self.level_cv_connected[channel] {
            self.level_cvs[channel][i].clamp(0.0, 2.0)
        } else {
            1.0
        }
    }

    #[inline]
    pub fn master_cv(&self, i: usize) -> f32 {
        if self.master_cv_connected {
            self.master_cv[i].clamp(0.0, 2.0)
        } else {
            1.0
        }
    }

    #[inline]
    pub fn pan_mod(&self, channel: usize, i: usize) -> f32 {
        if self.pan_mod_connected[channel] {
            self.pan_mods[channel][i].clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }
}
