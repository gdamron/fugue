//! Output state for the Reverb module.

pub const OUTPUTS: [&str; 2] = ["left", "right"];

pub struct ReverbOutputs {
    left: f32,
    right: f32,
}

impl ReverbOutputs {
    pub fn new() -> Self {
        Self {
            left: 0.0,
            right: 0.0,
        }
    }

    pub fn set(&mut self, left: f32, right: f32) {
        self.left = left;
        self.right = right;
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "left" => Ok(self.left),
            "right" => Ok(self.right),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for ReverbOutputs {
    fn default() -> Self {
        Self::new()
    }
}
