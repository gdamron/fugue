//! Output state for the Lfo module.

pub const OUTPUTS: [&str; 2] = ["out", "out_uni"];

pub struct LfoOutputs {
    out: f32,
    out_uni: f32,
}

impl LfoOutputs {
    pub fn new() -> Self {
        Self {
            out: 0.0,
            out_uni: 0.5,
        }
    }

    pub fn set_bipolar(&mut self, value: f32) {
        self.out = value;
        self.out_uni = (value + 1.0) * 0.5;
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "out" => Ok(self.out),
            "out_uni" => Ok(self.out_uni),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for LfoOutputs {
    fn default() -> Self {
        Self::new()
    }
}
