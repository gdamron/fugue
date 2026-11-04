//! Output state for the Clock module.

use crate::MAX_BLOCK;

/// The gate outputs, then `bpm`, the tempo as a signal.
pub const OUTPUTS: [&str; 6] = ["beat", "beat_d4", "beat_d2", "beat_x2", "beat_x4", "bpm"];

/// How many beats apart each gate output's gates rise, in [`OUTPUTS`]
/// order. `bpm` carries no gates, so it has no entry.
pub(super) const GATE_BEATS: [f64; 5] = [1.0, 4.0, 2.0, 0.5, 0.25];

pub struct ClockOutputs {
    beat: [f32; MAX_BLOCK],
    beat_d4: [f32; MAX_BLOCK],
    beat_d2: [f32; MAX_BLOCK],
    beat_x2: [f32; MAX_BLOCK],
    beat_x4: [f32; MAX_BLOCK],
    bpm: [f32; MAX_BLOCK],
}

impl ClockOutputs {
    pub fn new() -> Self {
        Self {
            beat: [0.0; MAX_BLOCK],
            beat_d4: [0.0; MAX_BLOCK],
            beat_d2: [0.0; MAX_BLOCK],
            beat_x2: [0.0; MAX_BLOCK],
            beat_x4: [0.0; MAX_BLOCK],
            bpm: [0.0; MAX_BLOCK],
        }
    }

    /// Writes all five beat outputs and the tempo for frame `i`.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn set_all(
        &mut self,
        i: usize,
        beat: f32,
        beat_d4: f32,
        beat_d2: f32,
        beat_x2: f32,
        beat_x4: f32,
        bpm: f32,
    ) {
        self.bpm[i] = bpm;
        self.beat[i] = beat;
        self.beat_d4[i] = beat_d4;
        self.beat_d2[i] = beat_d2;
        self.beat_x2[i] = beat_x2;
        self.beat_x4[i] = beat_x4;
    }

    /// Block buffer for the indexed output port. Index matches `OUTPUTS`.
    #[inline]
    pub fn block(&self, index: usize) -> &[f32] {
        match index {
            0 => &self.beat,
            1 => &self.beat_d4,
            2 => &self.beat_d2,
            3 => &self.beat_x2,
            4 => &self.beat_x4,
            _ => &self.bpm,
        }
    }

    pub fn get(&self, port: &str) -> Result<f32, String> {
        match port {
            "beat" => Ok(self.beat[0]),
            "beat_d4" => Ok(self.beat_d4[0]),
            "beat_d2" => Ok(self.beat_d2[0]),
            "beat_x2" => Ok(self.beat_x2[0]),
            "beat_x4" => Ok(self.beat_x4[0]),
            "bpm" => Ok(self.bpm[0]),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}

impl Default for ClockOutputs {
    fn default() -> Self {
        Self::new()
    }
}
