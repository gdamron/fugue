//! Input state for the SamplePlayer module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 3] = ["play", "loop", "pitch"];

pub struct SamplePlayerInputs {
    play: [f32; MAX_BLOCK],
    loop_enabled: [f32; MAX_BLOCK],
    loop_connected: bool,
    pitch: [f32; MAX_BLOCK],
    pitch_connected: bool,
}

impl SamplePlayerInputs {
    pub fn new() -> Self {
        Self {
            play: [0.0; MAX_BLOCK],
            loop_enabled: [0.0; MAX_BLOCK],
            loop_connected: false,
            pitch: [1.0; MAX_BLOCK],
            pitch_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "play" => {
                self.play.fill(value);
                Ok(())
            }
            "loop" => {
                self.loop_enabled.fill(value);
                self.loop_connected = true;
                Ok(())
            }
            "pitch" => {
                self.pitch.fill(value);
                self.pitch_connected = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.play,
            1 => &mut self.loop_enabled,
            _ => &mut self.pitch,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        match index {
            1 => self.loop_connected = connected,
            2 => self.pitch_connected = connected,
            _ => {}
        }
    }

    #[inline]
    pub fn play(&self, i: usize) -> f32 {
        self.play[i]
    }

    #[inline]
    pub fn loop_enabled(&self, i: usize, control: bool) -> bool {
        if self.loop_connected {
            self.loop_enabled[i] > 0.5
        } else {
            control
        }
    }

    #[inline]
    pub fn pitch(&self, i: usize, control: f32) -> f32 {
        if self.pitch_connected {
            self.pitch[i]
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
