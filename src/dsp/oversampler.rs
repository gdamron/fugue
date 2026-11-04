//! Running a nonlinear process at two or four times the sample rate.
//!
//! Distortion and waveshaping add harmonics, and harmonics above Nyquist
//! fold back as inharmonic aliases: the grainy, metallic edge of a cheap
//! distortion. Raising the rate first gives the new harmonics room above
//! the audible band, where the filter on the way back down removes them.

use super::halfband::HalfbandStage;
use crate::MAX_BLOCK;

/// Stopband rejection of every stage, with margin over the 70 dB the
/// images and aliases must be held below.
const ATTENUATION_DB: f64 = 90.0;
/// The top of the band kept flat, in hertz.
const PASSBAND_HZ: f64 = 18_000.0;
/// At low sample rates 18 kHz is near or past Nyquist: keep this fraction
/// of Nyquist instead.
const MAX_PASS_FRACTION: f64 = 0.8;

/// How much the rate rises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Factor {
    Two,
    Four,
}

impl Factor {
    /// The rate multiplier: 2 or 4.
    pub fn ratio(self) -> usize {
        match self {
            Self::Two => 2,
            Self::Four => 4,
        }
    }
}

/// Raises a block to 2× or 4× the sample rate and brings it back down,
/// with halfband filters flat to 18 kHz (within 0.1 dB) that hold images
/// and aliases at least 70 dB down.
///
/// Built on the control thread (it designs its filters and sizes its
/// buffers then); the audio thread runs it without allocating.
pub struct Oversampler {
    factor: Factor,
    /// Up: base rate → 2×, then 2× → 4×.
    up: [HalfbandStage; 2],
    /// Down: 4× → 2×, then 2× → base rate.
    down: [HalfbandStage; 2],
    /// The 2× signal between the two stages of a 4× round trip.
    middle: Box<[f32]>,
    /// The signal at the high rate, handed to [`Oversampler::process`]'s
    /// closure.
    high: Box<[f32]>,
}

impl Oversampler {
    /// An oversampler by `factor` for a signal at `sample_rate`, taking
    /// blocks of up to [`MAX_BLOCK`] frames.
    pub fn new(factor: Factor, sample_rate: u32) -> Self {
        let nyquist = f64::from(sample_rate.max(1)) / 2.0;
        let pass_hz = PASSBAND_HZ.min(MAX_PASS_FRACTION * nyquist);
        // Each stage's band edge as a fraction of its lower rate's Nyquist.
        let first = HalfbandStage::new(pass_hz / nyquist, ATTENUATION_DB);
        let second = HalfbandStage::new(pass_hz / (2.0 * nyquist), ATTENUATION_DB);
        Self {
            factor,
            up: [first.clone(), second.clone()],
            down: [second, first],
            middle: vec![0.0; 2 * MAX_BLOCK].into_boxed_slice(),
            high: vec![0.0; 4 * MAX_BLOCK].into_boxed_slice(),
        }
    }

    /// The oversampling factor.
    pub fn factor(&self) -> Factor {
        self.factor
    }

    /// The delay of a round trip (up, then down), in frames at the base
    /// rate. Not always a whole number of frames.
    pub fn latency(&self) -> f32 {
        let outer = self.up[0].round_trip_delay() as f32 / 2.0;
        match self.factor {
            Factor::Two => outer,
            Factor::Four => outer + self.up[1].round_trip_delay() as f32 / 4.0,
        }
    }

    /// Raises `input` to the high rate in `output`, which must be
    /// `factor` times as long.
    pub fn upsample(&mut self, input: &[f32], output: &mut [f32]) {
        upsample(self.factor, &mut self.up, &mut self.middle, input, output);
    }

    /// Brings `input` at the high rate back to the base rate in `output`,
    /// which must be `factor` times shorter.
    pub fn downsample(&mut self, input: &[f32], output: &mut [f32]) {
        downsample(self.factor, &mut self.down, &mut self.middle, input, output);
    }

    /// Runs `shape` on `block` at the high rate, in place: up, shape,
    /// down. The block comes back [`Oversampler::latency`] frames late.
    pub fn process(&mut self, block: &mut [f32], mut shape: impl FnMut(&mut [f32])) {
        let high = &mut self.high[..block.len() * self.factor.ratio()];
        upsample(self.factor, &mut self.up, &mut self.middle, block, high);
        shape(high);
        downsample(self.factor, &mut self.down, &mut self.middle, high, block);
    }

    /// Clears the filters' memory.
    pub fn reset(&mut self) {
        for stage in self.up.iter_mut().chain(&mut self.down) {
            stage.reset();
        }
    }
}

fn upsample(
    factor: Factor,
    stages: &mut [HalfbandStage; 2],
    middle: &mut [f32],
    input: &[f32],
    output: &mut [f32],
) {
    assert!(input.len() <= MAX_BLOCK && output.len() == input.len() * factor.ratio());
    match factor {
        Factor::Two => stages[0].upsample(input, output),
        Factor::Four => {
            let middle = &mut middle[..input.len() * 2];
            stages[0].upsample(input, middle);
            stages[1].upsample(middle, output);
        }
    }
}

fn downsample(
    factor: Factor,
    stages: &mut [HalfbandStage; 2],
    middle: &mut [f32],
    input: &[f32],
    output: &mut [f32],
) {
    assert!(output.len() <= MAX_BLOCK && input.len() == output.len() * factor.ratio());
    match factor {
        Factor::Two => stages[1].downsample(input, output),
        Factor::Four => {
            let middle = &mut middle[..output.len() * 2];
            stages[0].downsample(input, middle);
            stages[1].downsample(middle, output);
        }
    }
}

#[cfg(test)]
#[path = "oversampler_tests.rs"]
mod tests;
