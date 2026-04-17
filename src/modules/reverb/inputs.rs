//! Input state for the Reverb module.

pub const INPUTS: [&str; 2] = ["left", "right"];

pub struct ReverbInputs {
    left: f32,
    right: f32,
    right_active: bool,
}

impl ReverbInputs {
    pub fn new() -> Self {
        Self {
            left: 0.0,
            right: 0.0,
            right_active: false,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "left" => {
                self.left = value;
                Ok(())
            }
            "right" => {
                self.right = value;
                self.right_active = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn reset(&mut self) {
        self.right_active = false;
    }

    pub fn left(&self) -> f32 {
        self.left
    }

    pub fn right(&self) -> f32 {
        self.right
    }

    /// Returns true if the right input was set this sample.
    pub fn right_active(&self) -> bool {
        self.right_active
    }
}

impl Default for ReverbInputs {
    fn default() -> Self {
        Self::new()
    }
}
