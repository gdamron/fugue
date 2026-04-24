//! Input state for the Vca module.

pub const INPUTS: [&str; 2] = ["audio", "cv"];

pub struct VcaInputs {
    audio: f32,
    cv: f32,
    cv_active: bool,
}

impl VcaInputs {
    pub fn new() -> Self {
        Self {
            audio: 0.0,
            cv: 1.0,
            cv_active: false,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio = value;
                Ok(())
            }
            "cv" => {
                self.cv = value.clamp(0.0, 1.0);
                self.cv_active = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn reset(&mut self) {
        self.cv_active = false;
    }

    /// Hot-path indexed setter. Index must match `INPUTS` order.
    #[inline]
    pub fn set_by_index(&mut self, index: usize, value: f32) {
        match index {
            0 => self.audio = value,
            1 => {
                self.cv = value.clamp(0.0, 1.0);
                self.cv_active = true;
            }
            _ => {}
        }
    }

    pub fn audio(&self) -> f32 {
        self.audio
    }

    pub fn cv(&self, control: f32) -> f32 {
        if self.cv_active {
            self.cv
        } else {
            control
        }
    }
}

impl Default for VcaInputs {
    fn default() -> Self {
        Self::new()
    }
}
