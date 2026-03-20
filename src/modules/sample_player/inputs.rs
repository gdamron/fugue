//! Input state for the SamplePlayer module.

pub const INPUTS: [&str; 2] = ["play", "loop"];

pub struct SamplePlayerInputs {
    play: f32,
    loop_enabled: f32,
    loop_active: bool,
}

impl SamplePlayerInputs {
    pub fn new() -> Self {
        Self {
            play: 0.0,
            loop_enabled: 0.0,
            loop_active: false,
        }
    }

    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "play" => {
                self.play = value;
                Ok(())
            }
            "loop" => {
                self.loop_enabled = value;
                self.loop_active = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn reset(&mut self) {
        self.loop_active = false;
    }

    pub fn play(&self) -> f32 {
        self.play
    }

    pub fn loop_enabled(&self, control: bool) -> bool {
        if self.loop_active {
            self.loop_enabled > 0.5
        } else {
            control
        }
    }
}

impl Default for SamplePlayerInputs {
    fn default() -> Self {
        Self::new()
    }
}
