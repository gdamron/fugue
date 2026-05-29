//! Input state for the SamplePlayer module.

pub const INPUTS: [&str; 3] = ["play", "loop", "pitch"];

pub struct SamplePlayerInputs {
    play: f32,
    loop_enabled: f32,
    loop_active: bool,
    pitch: f32,
    pitch_active: bool,
}

impl SamplePlayerInputs {
    pub fn new() -> Self {
        Self {
            play: 0.0,
            loop_enabled: 0.0,
            loop_active: false,
            pitch: 1.0,
            pitch_active: false,
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
            "pitch" => {
                self.pitch = value;
                self.pitch_active = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    pub fn reset(&mut self) {
        self.loop_active = false;
        self.pitch_active = false;
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

    pub fn pitch(&self, control: f32) -> f32 {
        if self.pitch_active {
            self.pitch
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
