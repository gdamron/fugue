//! Input state for the Adsr module.

pub const INPUTS: [&str; 5] = ["gate", "attack", "decay", "sustain", "release"];

pub struct AdsrInputs {
    gate: f32,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    attack_active: bool,
    decay_active: bool,
    sustain_active: bool,
    release_active: bool,
}

impl AdsrInputs {
    pub fn new() -> Self {
        Self {
            gate: 0.0,
            attack: 0.0,
            decay: 0.0,
            sustain: 0.0,
            release: 0.0,
            attack_active: false,
            decay_active: false,
            sustain_active: false,
            release_active: false,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => {
                self.gate = value;
                Ok(())
            }
            "attack" => {
                self.attack = value.max(0.0);
                self.attack_active = true;
                Ok(())
            }
            "decay" => {
                self.decay = value.max(0.0);
                self.decay_active = true;
                Ok(())
            }
            "sustain" => {
                self.sustain = value.clamp(0.0, 1.0);
                self.sustain_active = true;
                Ok(())
            }
            "release" => {
                self.release = value.max(0.0);
                self.release_active = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn reset(&mut self) {
        self.attack_active = false;
        self.decay_active = false;
        self.sustain_active = false;
        self.release_active = false;
    }

    /// Hot-path indexed setter. Index must match `INPUTS` order.
    #[inline]
    pub fn set_by_index(&mut self, index: usize, value: f32) {
        match index {
            0 => self.gate = value,
            1 => {
                self.attack = value.max(0.0);
                self.attack_active = true;
            }
            2 => {
                self.decay = value.max(0.0);
                self.decay_active = true;
            }
            3 => {
                self.sustain = value.clamp(0.0, 1.0);
                self.sustain_active = true;
            }
            4 => {
                self.release = value.max(0.0);
                self.release_active = true;
            }
            _ => {}
        }
    }

    pub fn gate(&self) -> f32 {
        self.gate
    }

    pub fn attack(&self, control: f32) -> f32 {
        if self.attack_active {
            self.attack
        } else {
            control
        }
    }

    pub fn decay(&self, control: f32) -> f32 {
        if self.decay_active {
            self.decay
        } else {
            control
        }
    }

    pub fn sustain(&self, control: f32) -> f32 {
        if self.sustain_active {
            self.sustain
        } else {
            control
        }
    }

    pub fn release(&self, control: f32) -> f32 {
        if self.release_active {
            self.release
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
