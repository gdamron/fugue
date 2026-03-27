//! Input state for the Mixer module.

use super::MAX_CHANNELS;

pub static INPUT_NAMES: [&str; MAX_CHANNELS] =
    ["in1", "in2", "in3", "in4", "in5", "in6", "in7", "in8"];
pub static LEVEL_NAMES: [&str; MAX_CHANNELS] = [
    "level1", "level2", "level3", "level4", "level5", "level6", "level7", "level8",
];
pub static PAN_NAMES: [&str; MAX_CHANNELS] = [
    "pan1", "pan2", "pan3", "pan4", "pan5", "pan6", "pan7", "pan8",
];

pub struct MixerInputs {
    names: Vec<&'static str>,
    audio: [f32; MAX_CHANNELS],
    level_cvs: [f32; MAX_CHANNELS],
    pan_mods: [f32; MAX_CHANNELS],
    master_cv: f32,
    level_cv_active: [bool; MAX_CHANNELS],
    pan_mod_active: [bool; MAX_CHANNELS],
    master_cv_active: bool,
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
            audio: [0.0; MAX_CHANNELS],
            level_cvs: [1.0; MAX_CHANNELS],
            pan_mods: [0.0; MAX_CHANNELS],
            master_cv: 1.0,
            level_cv_active: [false; MAX_CHANNELS],
            pan_mod_active: [false; MAX_CHANNELS],
            master_cv_active: false,
        }
    }

    pub fn names(&self) -> &[&str] {
        &self.names
    }

    pub fn set(&mut self, channels: usize, port: &str, value: f32) -> Result<(), String> {
        if let Some(rest) = port.strip_prefix("in") {
            if let Ok(num) = rest.parse::<usize>() {
                let idx = num - 1;
                if idx < channels {
                    self.audio[idx] = value;
                    return Ok(());
                }
            }
        }

        if let Some(rest) = port.strip_prefix("level") {
            if let Ok(num) = rest.parse::<usize>() {
                let idx = num - 1;
                if idx < channels {
                    self.level_cvs[idx] = value.clamp(0.0, 2.0);
                    self.level_cv_active[idx] = true;
                    return Ok(());
                }
            }
        }

        if let Some(rest) = port.strip_prefix("pan") {
            if let Ok(num) = rest.parse::<usize>() {
                let idx = num - 1;
                if idx < channels {
                    self.pan_mods[idx] = value.clamp(-1.0, 1.0);
                    self.pan_mod_active[idx] = true;
                    return Ok(());
                }
            }
        }

        if port == "master" {
            self.master_cv = value.clamp(0.0, 2.0);
            self.master_cv_active = true;
            return Ok(());
        }

        Err(format!("Unknown input port: {}", port))
    }

    pub fn reset(&mut self) {
        self.level_cv_active = [false; MAX_CHANNELS];
        self.pan_mod_active = [false; MAX_CHANNELS];
        self.master_cv_active = false;
    }

    pub fn audio(&self, channel: usize) -> f32 {
        self.audio[channel]
    }

    pub fn level_cv(&self, channel: usize) -> f32 {
        if self.level_cv_active[channel] {
            self.level_cvs[channel]
        } else {
            1.0
        }
    }

    pub fn master_cv(&self) -> f32 {
        if self.master_cv_active {
            self.master_cv
        } else {
            1.0
        }
    }

    pub fn pan_mod(&self, channel: usize) -> f32 {
        if self.pan_mod_active[channel] {
            self.pan_mods[channel]
        } else {
            0.0
        }
    }
}
