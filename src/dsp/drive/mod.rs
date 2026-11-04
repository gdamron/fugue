//! A complete drive: tone, shape (oversampled for curves), DC blocker,
//! tone, then the dry/wet mix and level.
//!
//! From a little warmth on a bass to a fuzz or a crushed, early-sampler
//! grit. Every setting glides and every change of shape crossfades, so a
//! processor can be played live, or sit in a feedback loop, without a
//! click.

use crate::dsp::{quantize, DcBlocker, DelayLine, Factor, LinearRamp, Oversampler, SampleHold};
use crate::dsp::{Curve, Tilt, Waveshaper};
use crate::MAX_BLOCK;

use self::calibration::Calibration;
pub use self::calibration::REFERENCE_PEAK;

mod calibration;
#[cfg(test)]
mod tests;

/// How long a change of shape crossfades, in seconds.
const SHAPE_FADE_SECONDS: f32 = 0.02;
/// The curve shapes come first in [`DriveShape::ALL`].
const CURVE_COUNT: usize = 4;

/// What a drive does to the signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveShape {
    /// Tanh-like saturation ([`Curve::Soft`]).
    Soft,
    /// Clipping ([`Curve::Hard`]).
    Hard,
    /// Foldback ([`Curve::Fold`]).
    Fold,
    /// Even harmonics too ([`Curve::Asymmetric`]).
    Asymmetric,
    /// Fewer bits: drive coarsens the grid from 16 bits to 3.
    Bits,
    /// A lower rate: drive holds each sample for 1 to 64 frames (counted
    /// in frames, so it sounds coarser at lower sample rates).
    Rate,
}

impl DriveShape {
    /// The number of shapes.
    pub const COUNT: usize = 6;

    /// Every shape, curves first.
    pub const ALL: [DriveShape; Self::COUNT] = [
        Self::Soft,
        Self::Hard,
        Self::Fold,
        Self::Asymmetric,
        Self::Bits,
        Self::Rate,
    ];

    /// Its position in [`DriveShape::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// The curve it runs, for the waveshaping shapes.
    pub fn curve(self) -> Option<Curve> {
        match self {
            Self::Soft => Some(Curve::Soft),
            Self::Hard => Some(Curve::Hard),
            Self::Fold => Some(Curve::Fold),
            Self::Asymmetric => Some(Curve::Asymmetric),
            Self::Bits | Self::Rate => None,
        }
    }
}

/// One frame's settings, as the glides stand.
#[derive(Clone, Copy, Default)]
struct Frame {
    drive: f32,
    bias: f32,
    auto_level: f32,
    weights: [f32; DriveShape::COUNT],
    /// For each active curve: the gain in front of it, the level after it,
    /// and what the bias alone would leave (taken off as a constant).
    curve_gain: [f32; CURVE_COUNT],
    curve_level: [f32; CURVE_COUNT],
    curve_rest: [f32; CURVE_COUNT],
}

/// Saturation, distortion and bit or rate reduction of one signal.
///
/// Curves run oversampled (and anti-aliased at their corners), so even hard
/// settings stay free of aliasing; bits and rate reduction alias on purpose
/// and run at the base rate, delayed to stay in line. The output is within
/// ±1 and trails the input by [`DriveProcessor::latency`] frames; the dry
/// share is delayed to match, so the mix never combs.
///
/// Built on a control thread (it sizes its buffers then); the audio thread
/// runs it without allocating.
pub struct DriveProcessor {
    calibration: &'static Calibration,
    oversampler: Option<Oversampler>,
    latency: usize,
    pre: Tilt,
    post: Tilt,
    drive: LinearRamp,
    bias: LinearRamp,
    auto_level: LinearRamp,
    level: LinearRamp,
    mix: LinearRamp,
    /// Each shape's share of the output: a change of shape crossfades.
    weights: [LinearRamp; DriveShape::COUNT],
    shape: DriveShape,
    active: [bool; DriveShape::COUNT],
    shapers: [Waveshaper; CURVE_COUNT],
    hold: SampleHold,
    /// What bits take off for the bias, gliding: the grid snaps the bias to
    /// a step, and a snap of the offset would click.
    bits_offset: LinearRamp,
    dc: DcBlocker,
    /// The input, delayed to line up with the oversampled path.
    dry_line: DelayLine,
    /// The toned input, delayed the same, for bits and rate.
    crush_line: DelayLine,
    frames: Box<[Frame]>,
    work: Box<[f32]>,
    dry: Box<[f32]>,
    crush: Box<[f32]>,
}

impl DriveProcessor {
    /// A soft drive at `sample_rate` with drive 0, everything else neutral
    /// (flat tone, no bias, full level and mix, automatic level on),
    /// oversampling curves by `factor` (`None` runs them at the base rate),
    /// its settings gliding over `glide_frames`.
    pub fn new(sample_rate: u32, factor: Option<Factor>, glide_frames: u32) -> Self {
        let oversampler = factor.map(|factor| Oversampler::new(factor, sample_rate));
        let latency = oversampler.as_ref().map_or(0, Oversampler::latency);
        let fade = (SHAPE_FADE_SECONDS * sample_rate as f32).round() as u32;
        let shapers =
            [Curve::Soft, Curve::Hard, Curve::Fold, Curve::Asymmetric].map(Waveshaper::new);
        let mut weights = [LinearRamp::new(0.0, fade); DriveShape::COUNT];
        weights[0] = LinearRamp::new(1.0, fade);
        Self {
            calibration: Calibration::get(),
            oversampler,
            latency,
            pre: Tilt::new(sample_rate, glide_frames),
            post: Tilt::new(sample_rate, glide_frames),
            drive: LinearRamp::new(0.0, glide_frames),
            bias: LinearRamp::new(0.0, glide_frames),
            auto_level: LinearRamp::new(1.0, glide_frames),
            level: LinearRamp::new(1.0, glide_frames),
            mix: LinearRamp::new(1.0, glide_frames),
            weights,
            shape: DriveShape::Soft,
            active: [false; DriveShape::COUNT],
            shapers,
            hold: SampleHold::new(),
            bits_offset: LinearRamp::new(0.0, glide_frames),
            dc: DcBlocker::new(sample_rate),
            dry_line: DelayLine::new(latency + 1),
            crush_line: DelayLine::new(latency + 1),
            frames: vec![Frame::default(); MAX_BLOCK].into_boxed_slice(),
            work: vec![0.0; MAX_BLOCK].into_boxed_slice(),
            dry: vec![0.0; MAX_BLOCK].into_boxed_slice(),
            crush: vec![0.0; MAX_BLOCK].into_boxed_slice(),
        }
    }

    /// How many frames the output trails the input.
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// How hard the shape is pushed, 0 to 1 (clamped). For curves, the gain
    /// in front of them, 0 dB to +24 dB (+18 dB for fold); for bits, 16
    /// bits to 3; for rate, every sample kept to one in 64.
    pub fn set_drive(&mut self, drive: f32) {
        self.drive.set_target(unit(drive));
    }

    /// Crossfades to `shape` over 20 ms.
    pub fn set_shape(&mut self, shape: DriveShape) {
        if shape == self.shape {
            return;
        }
        self.shape = shape;
        for (n, weight) in self.weights.iter_mut().enumerate() {
            weight.set_target(if n == shape.index() { 1.0 } else { 0.0 });
        }
    }

    /// The tilt before the shape, −1 (dark) to 1 (bright).
    pub fn set_pre_tone(&mut self, tilt: f32) {
        self.pre.set_tilt(tilt);
    }

    /// The tilt after the shape, −1 (dark) to 1 (bright).
    pub fn set_post_tone(&mut self, tilt: f32) {
        self.post.set_tilt(tilt);
    }

    /// An offset into the curves and bits, −1 to 1 (clamped): off centre
    /// adds even harmonics. The offset it leaves is removed. Rate ignores
    /// it.
    pub fn set_bias(&mut self, bias: f32) {
        self.bias.set_target(signed(bias));
    }

    /// The output level, 0 to 1 (clamped).
    pub fn set_level(&mut self, level: f32) {
        self.level.set_target(unit(level));
    }

    /// Whether a −12 dBFS signal keeps its level as drive moves.
    pub fn set_auto_level(&mut self, on: bool) {
        self.auto_level.set_target(if on { 1.0 } else { 0.0 });
    }

    /// The driven share of the output, 0 (dry) to 1 (clamped).
    pub fn set_mix(&mut self, mix: f32) {
        self.mix.set_target(unit(mix));
    }

    /// Ends every glide and crossfade at once: where a processor that was
    /// just set up starts.
    pub fn settle(&mut self) {
        self.pre.settle();
        self.post.settle();
        let ramps = [
            &mut self.drive,
            &mut self.bias,
            &mut self.auto_level,
            &mut self.level,
            &mut self.mix,
            &mut self.bits_offset,
        ];
        for ramp in ramps.into_iter().chain(&mut self.weights) {
            ramp.settle();
        }
    }

    /// Drives `input` into `output`, over as many frames as the shorter
    /// holds (at most [`MAX_BLOCK`]). Input that is not finite is taken as
    /// silence.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let frames = input.len().min(output.len()).min(MAX_BLOCK);
        if frames == 0 {
            return;
        }
        self.read_settings(&input[..frames]);
        let any_curve = self.active[..CURVE_COUNT].contains(&true);
        if any_curve {
            self.shape_curves(frames);
        }
        let (bits, rate) = (DriveShape::Bits, DriveShape::Rate);
        for (i, out) in output[..frames].iter_mut().enumerate() {
            let frame = self.frames[i];
            let mut wet = if any_curve { self.work[i] } else { 0.0 };
            if self.active[bits.index()] {
                let (step, compensation) = self.calibration.at(bits, frame.drive);
                let gain = 1.0 + frame.auto_level * (compensation - 1.0);
                self.bits_offset.set_target(quantize(frame.bias, step));
                let offset = self.bits_offset.next_value();
                let shaped = quantize(self.crush[i] + frame.bias, step) - offset;
                wet += frame.weights[bits.index()] * gain * shaped;
            }
            if self.active[rate.index()] {
                let (kept, _) = self.calibration.at(rate, frame.drive);
                wet += frame.weights[rate.index()] * self.hold.process(self.crush[i], kept);
            }
            let wet = self.post.process(self.dc.process(wet));
            let dry = self.dry[i];
            let mix = self.mix.next_value();
            let level = self.level.next_value();
            *out = (level * (dry + mix * (wet - dry))).clamp(-1.0, 1.0);
        }
    }

    /// Steps every glide through the block, tones the input, and lines up
    /// the dry and lo-fi paths with the oversampled one.
    fn read_settings(&mut self, input: &[f32]) {
        let mut active = [false; DriveShape::COUNT];
        for (i, &x) in input.iter().enumerate() {
            let x = if x.is_finite() { x } else { 0.0 };
            let toned = self.pre.process(x);
            self.dry_line.write_and_advance(x);
            self.crush_line.write_and_advance(toned);
            self.dry[i] = self.dry_line.read(self.latency + 1);
            self.crush[i] = self.crush_line.read(self.latency + 1);
            self.work[i] = toned;
            let mut frame = Frame {
                drive: self.drive.next_value(),
                bias: self.bias.next_value(),
                auto_level: self.auto_level.next_value(),
                ..Frame::default()
            };
            for (n, weight) in self.weights.iter_mut().enumerate() {
                frame.weights[n] = weight.next_value();
                active[n] |= frame.weights[n] != 0.0;
            }
            for (n, shape) in DriveShape::ALL[..CURVE_COUNT].iter().enumerate() {
                if frame.weights[n] == 0.0 {
                    continue;
                }
                let (gain, compensation) = self.calibration.at(*shape, frame.drive);
                frame.curve_gain[n] = gain;
                frame.curve_level[n] = 1.0 + frame.auto_level * (compensation - 1.0);
                frame.curve_rest[n] = self.shapers[n].curve().apply(frame.bias);
            }
            self.frames[i] = frame;
        }
        // A shape coming back in starts from rest, not from where it left;
        // so do the oversampler's filters, which stopped mid-tail when no
        // curve was left to run.
        let curves_back =
            active[..CURVE_COUNT].contains(&true) && !self.active[..CURVE_COUNT].contains(&true);
        if curves_back {
            if let Some(oversampler) = self.oversampler.as_mut() {
                oversampler.reset();
            }
        }
        for shape in DriveShape::ALL {
            let n = shape.index();
            if !active[n] || self.active[n] {
                continue;
            }
            match shape {
                DriveShape::Rate => self.hold.reset(),
                DriveShape::Bits => {
                    let first = self.frames[0];
                    let (step, _) = self.calibration.at(shape, first.drive);
                    self.bits_offset.snap(quantize(first.bias, step));
                }
                _ => self.shapers[n].rest_at(self.frames[0].bias),
            }
        }
        self.active = active;
    }

    /// Runs the active curves over the toned block in `work`, oversampled
    /// when the processor has an oversampler.
    fn shape_curves(&mut self, frames: usize) {
        let settings = &self.frames[..frames];
        let active = self.active;
        let shapers = &mut self.shapers;
        let ratio = self.oversampler.as_ref().map_or(1, |o| o.factor().ratio());
        let mut shape = |high: &mut [f32]| {
            for (j, sample) in high.iter_mut().enumerate() {
                let frame = &settings[j / ratio];
                let x = *sample;
                let mut y = 0.0;
                for (n, shaper) in shapers.iter_mut().enumerate() {
                    if !active[n] {
                        continue;
                    }
                    // The bias alone would leave a constant: take it off
                    // here, and the DC blocker takes what moves.
                    let shaped = shaper.process(frame.curve_gain[n] * x + frame.bias);
                    y += frame.weights[n] * frame.curve_level[n] * (shaped - frame.curve_rest[n]);
                }
                *sample = y;
            }
        };
        let block = &mut self.work[..frames];
        match self.oversampler.as_mut() {
            Some(oversampler) => oversampler.process(block, shape),
            None => shape(block),
        }
    }
}

/// `value` within 0 to 1; not a number is 0.
fn unit(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// `value` within ±1; not a number is 0.
fn signed(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    }
}
