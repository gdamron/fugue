//! Input state for the SampleInstrument module.

use crate::MAX_BLOCK;

pub const INPUTS: [&str; 3] = ["frequency", "gate", "velocity"];

pub struct SampleInstrumentInputs {
    frequency: [f32; MAX_BLOCK],
    gate: [f32; MAX_BLOCK],
    velocity: [f32; MAX_BLOCK],
    velocity_connected: bool,
}

impl SampleInstrumentInputs {
    pub fn new() -> Self {
        Self {
            frequency: [0.0; MAX_BLOCK],
            gate: [0.0; MAX_BLOCK],
            velocity: [1.0; MAX_BLOCK],
            velocity_connected: false,
        }
    }

    /// Fills an input port's buffer with a constant value (control thread / tests).
    pub fn set(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "frequency" => {
                self.frequency.fill(value);
                Ok(())
            }
            "gate" => {
                self.gate.fill(value);
                Ok(())
            }
            "velocity" => {
                self.velocity.fill(value);
                self.velocity_connected = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    /// Mutable block buffer for the indexed input port. Index matches `INPUTS`.
    #[inline]
    pub fn block_mut(&mut self, index: usize) -> &mut [f32] {
        match index {
            0 => &mut self.frequency,
            1 => &mut self.gate,
            _ => &mut self.velocity,
        }
    }

    /// Records whether an input port is fed by an upstream connection.
    pub fn set_connected(&mut self, index: usize, connected: bool) {
        if index == 2 {
            self.velocity_connected = connected;
        }
    }

    #[inline]
    pub fn frequency(&self, i: usize) -> f32 {
        self.frequency[i]
    }

    #[inline]
    pub fn gate(&self, i: usize) -> f32 {
        self.gate[i]
    }

    /// Note velocity at frame `i`; full velocity when nothing is patched in.
    #[inline]
    pub fn velocity(&self, i: usize) -> f32 {
        if self.velocity_connected {
            self.velocity[i]
        } else {
            1.0
        }
    }
}

impl Default for SampleInstrumentInputs {
    fn default() -> Self {
        Self::new()
    }
}
