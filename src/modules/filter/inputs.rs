//! Input state for the Filter module.

pub const INPUTS: [&str; 4] = ["audio", "cutoff", "cutoff_cv", "resonance"];

pub struct FilterInputs {
    audio: f32,
    cutoff: f32,
    cutoff_cv: f32,
    resonance: f32,
    cutoff_active: bool,
    resonance_active: bool,
}

impl FilterInputs {
    pub fn new() -> Self {
        Self {
            audio: 0.0,
            cutoff: 0.0,
            cutoff_cv: 0.0,
            resonance: 0.0,
            cutoff_active: false,
            resonance_active: false,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio = value;
                Ok(())
            }
            "cutoff" => {
                self.cutoff = value.clamp(20.0, 20000.0);
                self.cutoff_active = true;
                Ok(())
            }
            "cutoff_cv" => {
                self.cutoff_cv = value;
                Ok(())
            }
            "resonance" => {
                self.resonance = value.clamp(0.0, 1.0);
                self.resonance_active = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Hot-path indexed setter. Index must match `INPUTS` order.
    #[inline]
    pub fn set_by_index(&mut self, index: usize, value: f32) {
        match index {
            0 => self.audio = value,
            1 => {
                self.cutoff = value.clamp(20.0, 20000.0);
                self.cutoff_active = true;
            }
            2 => self.cutoff_cv = value,
            3 => {
                self.resonance = value.clamp(0.0, 1.0);
                self.resonance_active = true;
            }
            _ => {}
        }
    }

    pub fn reset(&mut self) {
        self.cutoff_active = false;
        self.resonance_active = false;
    }

    pub fn audio(&self) -> f32 {
        self.audio
    }

    pub fn cutoff(&self, control: f32) -> f32 {
        if self.cutoff_active {
            self.cutoff
        } else {
            control
        }
    }

    pub fn cutoff_cv(&self) -> f32 {
        self.cutoff_cv
    }

    pub fn resonance(&self, control: f32) -> f32 {
        if self.resonance_active {
            self.resonance
        } else {
            control
        }
    }
}

impl Default for FilterInputs {
    fn default() -> Self {
        Self::new()
    }
}
