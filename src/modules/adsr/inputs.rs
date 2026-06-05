//! Input state for the Adsr module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 5] = ["gate", "attack", "decay", "sustain", "release"];

pub struct AdsrInputs {
    gate: [f32; MAX_BLOCK],
    attack: [f32; MAX_BLOCK],
    decay: [f32; MAX_BLOCK],
    sustain: [f32; MAX_BLOCK],
    release: [f32; MAX_BLOCK],
    attack_connected: bool,
    decay_connected: bool,
    sustain_connected: bool,
    release_connected: bool,
}

impl AdsrInputs {
    pub fn new() -> Self {
        Self {
            gate: [0.0; MAX_BLOCK],
            attack: [0.0; MAX_BLOCK],
            decay: [0.0; MAX_BLOCK],
            sustain: [0.0; MAX_BLOCK],
            release: [0.0; MAX_BLOCK],
            attack_connected: false,
            decay_connected: false,
            sustain_connected: false,
            release_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => {
                self.gate.fill(value);
                Ok(())
            }
            "attack" => {
                self.attack.fill(value.max(0.0));
                self.attack_connected = true;
                Ok(())
            }
            "decay" => {
                self.decay.fill(value.max(0.0));
                self.decay_connected = true;
                Ok(())
            }
            "sustain" => {
                self.sustain.fill(value.clamp(0.0, 1.0));
                self.sustain_connected = true;
                Ok(())
            }
            "release" => {
                self.release.fill(value.max(0.0));
                self.release_connected = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.gate,
            1 => &mut self.attack,
            2 => &mut self.decay,
            3 => &mut self.sustain,
            _ => &mut self.release,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        match index {
            1 => self.attack_connected = connected,
            2 => self.decay_connected = connected,
            3 => self.sustain_connected = connected,
            4 => self.release_connected = connected,
            _ => {}
        }
    }

    #[inline]
    pub fn gate(&self, i: usize) -> f32 {
        self.gate[i]
    }

    #[inline]
    pub fn attack(&self, i: usize, control: f32) -> f32 {
        if self.attack_connected {
            self.attack[i].max(0.0)
        } else {
            control
        }
    }

    #[inline]
    pub fn decay(&self, i: usize, control: f32) -> f32 {
        if self.decay_connected {
            self.decay[i].max(0.0)
        } else {
            control
        }
    }

    #[inline]
    pub fn sustain(&self, i: usize, control: f32) -> f32 {
        if self.sustain_connected {
            self.sustain[i].clamp(0.0, 1.0)
        } else {
            control
        }
    }

    #[inline]
    pub fn release(&self, i: usize, control: f32) -> f32 {
        if self.release_connected {
            self.release[i].max(0.0)
        } else {
            control
        }
    }
}

impl Default for AdsrInputs {
    fn default() -> Self {
        Self::new()
    }
}
