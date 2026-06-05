//! Input state for the Mixer module.

use super::MAX_CHANNELS;
use crate::MAX_BLOCK;

macro_rules! mixer_port_names {
    ($prefix:literal) => {
        [
            concat!($prefix, "1"),
            concat!($prefix, "2"),
            concat!($prefix, "3"),
            concat!($prefix, "4"),
            concat!($prefix, "5"),
            concat!($prefix, "6"),
            concat!($prefix, "7"),
            concat!($prefix, "8"),
            concat!($prefix, "9"),
            concat!($prefix, "10"),
            concat!($prefix, "11"),
            concat!($prefix, "12"),
            concat!($prefix, "13"),
            concat!($prefix, "14"),
            concat!($prefix, "15"),
            concat!($prefix, "16"),
            concat!($prefix, "17"),
            concat!($prefix, "18"),
            concat!($prefix, "19"),
            concat!($prefix, "20"),
            concat!($prefix, "21"),
            concat!($prefix, "22"),
            concat!($prefix, "23"),
            concat!($prefix, "24"),
            concat!($prefix, "25"),
            concat!($prefix, "26"),
            concat!($prefix, "27"),
            concat!($prefix, "28"),
            concat!($prefix, "29"),
            concat!($prefix, "30"),
            concat!($prefix, "31"),
            concat!($prefix, "32"),
            concat!($prefix, "33"),
            concat!($prefix, "34"),
            concat!($prefix, "35"),
            concat!($prefix, "36"),
            concat!($prefix, "37"),
            concat!($prefix, "38"),
            concat!($prefix, "39"),
            concat!($prefix, "40"),
            concat!($prefix, "41"),
            concat!($prefix, "42"),
            concat!($prefix, "43"),
            concat!($prefix, "44"),
            concat!($prefix, "45"),
            concat!($prefix, "46"),
            concat!($prefix, "47"),
            concat!($prefix, "48"),
            concat!($prefix, "49"),
            concat!($prefix, "50"),
            concat!($prefix, "51"),
            concat!($prefix, "52"),
            concat!($prefix, "53"),
            concat!($prefix, "54"),
            concat!($prefix, "55"),
            concat!($prefix, "56"),
            concat!($prefix, "57"),
            concat!($prefix, "58"),
            concat!($prefix, "59"),
            concat!($prefix, "60"),
            concat!($prefix, "61"),
            concat!($prefix, "62"),
            concat!($prefix, "63"),
            concat!($prefix, "64"),
        ]
    };
}

pub static INPUT_NAMES: [&str; MAX_CHANNELS] = mixer_port_names!("in");
pub static LEVEL_NAMES: [&str; MAX_CHANNELS] = mixer_port_names!("level");
pub static PAN_NAMES: [&str; MAX_CHANNELS] = mixer_port_names!("pan");

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
        for name in INPUT_NAMES.iter().take(channels) {
            names.push(*name);
        }
        for name in LEVEL_NAMES.iter().take(channels) {
            names.push(*name);
        }
        for name in PAN_NAMES.iter().take(channels) {
            names.push(*name);
        }
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
        if let Some(rest) = port.strip_prefix("in") {
            if let Ok(num) = rest.parse::<usize>() {
                let idx = num - 1;
                if idx < channels {
                    self.audio[idx].fill(value);
                    return Ok(());
                }
            }
        }

        if let Some(rest) = port.strip_prefix("level") {
            if let Ok(num) = rest.parse::<usize>() {
                let idx = num - 1;
                if idx < channels {
                    self.level_cvs[idx].fill(value.clamp(0.0, 2.0));
                    self.level_cv_connected[idx] = true;
                    return Ok(());
                }
            }
        }

        if let Some(rest) = port.strip_prefix("pan") {
            if let Ok(num) = rest.parse::<usize>() {
                let idx = num - 1;
                if idx < channels {
                    self.pan_mods[idx].fill(value.clamp(-1.0, 1.0));
                    self.pan_mod_connected[idx] = true;
                    return Ok(());
                }
            }
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
    ///   `[0, N)` → audio inputs (in1..inN)
    ///   `[N, 2N)` → level CVs (level1..levelN)
    ///   `[2N, 3N)` → pan mods (pan1..panN)
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
