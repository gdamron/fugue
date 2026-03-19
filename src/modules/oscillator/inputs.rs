//! Input state for the Oscillator module.

pub const INPUTS: [&str; 3] = ["frequency", "fm", "am"];

pub struct OscillatorInputs {
    frequency: f32,
    fm: f32,
    am: f32,
    frequency_active: bool,
}

impl OscillatorInputs {
    pub fn new() -> Self {
        Self {
            frequency: 0.0,
            fm: 0.0,
            am: 0.0,
            frequency_active: false,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "frequency" => {
                self.frequency = value;
                self.frequency_active = true;
                Ok(())
            }
            "fm" => {
                self.fm = value;
                Ok(())
            }
            "am" => {
                self.am = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn reset(&mut self) {
        self.frequency_active = false;
    }

    pub fn frequency(&self, control: f32) -> f32 {
        if self.frequency_active {
            self.frequency
        } else {
            control
        }
    }

    pub fn fm(&self) -> f32 {
        self.fm
    }

    pub fn am(&self) -> f32 {
        self.am
    }
}

impl Default for OscillatorInputs {
    fn default() -> Self {
        Self::new()
    }
}
