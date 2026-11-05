//! Ported from Mutable Instruments' `tides2/poly_slope_generator.h` (MIT,
//! Copyright 2017 Emilie Gillet), adapted to Fugue: one sample at a time,
//! with outputs scaled to −1..1 (cycles) and 0..1 (envelopes and gates).
//! See THIRD_PARTY_NOTICES.md.
//!
//! The [`Generator`] turns the ramp into the four outputs of the chosen
//! output mode, shaped by `shape` and smoothed or folded by `smoothness`.

use super::ramp::{Drive, RampGenerator, CHANNELS, MAX_FREQUENCY};
use super::ratios::{set, SetPicker};
use super::shaper::{RampShaper, Waveshaper};
use super::tables::{Tables, AUDIO_CURVES};
use super::{OutputMode, RampMode, Range};

/// The modes a generator runs in. A change crossfades between two
/// generators, so a generator's own modes never change mid-sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Modes {
    pub ramp: RampMode,
    pub output: OutputMode,
    pub range: Range,
}

/// One sample's settings, each 0..1 except `frequency` (over the sample
/// rate).
#[derive(Clone, Copy, Debug)]
pub(super) struct Params {
    pub frequency: f32,
    pub shape: f32,
    pub slope: f32,
    pub smoothness: f32,
    pub shift: f32,
}

/// The linear control curve, which turns an attack-decay phase into a
/// plain rise and fall.
const LINEAR_CONTROL_CURVE: usize = AUDIO_CURVES + 3;

#[derive(Clone, Copy, Debug)]
pub(super) struct Generator {
    pub modes: Modes,
    ramp: RampGenerator,
    shapers: [RampShaper; CHANNELS],
    waveshapers: [Waveshaper; CHANNELS],
    /// The two one-pole stages `smoothness` below 0.5 runs each output
    /// through.
    smooth: [[f32; 2]; CHANNELS],
    picker: SetPicker,
}

impl Generator {
    pub(super) fn new(modes: Modes) -> Self {
        Self {
            modes,
            ramp: RampGenerator::at_rest(),
            shapers: [RampShaper::default(); CHANNELS],
            waveshapers: [Waveshaper::default(); CHANNELS],
            smooth: [[0.0; 2]; CHANNELS],
            picker: SetPicker::default(),
        }
    }

    /// A copy of this generator that carries on in `modes`, from where
    /// this one is, for a crossfade from one to the other.
    pub(super) fn retuned(&self, modes: Modes) -> Self {
        let mut next = *self;
        next.modes = modes;
        next.ramp.align();
        next
    }

    /// The four outputs for the next sample.
    #[inline]
    pub(super) fn render(
        &mut self,
        tables: &Tables,
        params: &Params,
        drive: Drive,
    ) -> [f32; CHANNELS] {
        let Modes {
            ramp: mode,
            output,
            range,
        } = self.modes;
        let mut frequency = params.frequency.clamp(0.0, MAX_FREQUENCY);
        let mut pw = params.slope;
        if range == Range::Control && pw < 0.5 {
            // More of the knob for short attacks.
            pw = 0.5 + 0.6 * (pw - 0.5) / ((pw - 0.5).abs() + 0.1);
        }
        if drive.clock.is_some() && mode == RampMode::Ar {
            // A clock-driven AR becomes a trapezoid as the slope skews.
            frequency *= 1.0 + 2.0 * (pw - 0.5).abs();
        }
        // At high frequencies, ease off shaping and folding so the
        // harmonics they add stay below Nyquist.
        let steepness = 3.0 + (pw - 0.5).abs() * 5.0;
        let shape_amount = (params.shape - 0.5).abs() * 2.0;
        let attenuation = tame(frequency, steepness, 16.0);
        let shape = 0.5 + (params.shape - 0.5) * attenuation;
        let mut smoothness = params.smoothness;
        if smoothness > 0.5 {
            let harmonics = steepness * (3.0 + shape_amount * attenuation * 5.0);
            smoothness = 0.5 + (smoothness - 0.5) * tame(frequency, harmonics, 12.0);
        }
        let is_phasor = !(range == Range::Audio && mode == RampMode::Looping);
        let shift = 2.0 * params.shift - 1.0;
        let fold = (2.0 * (smoothness - 0.5)).max(0.0);
        let (curve, blend) = if is_phasor {
            split(AUDIO_CURVES as f32 + shape * 5.9999)
        } else {
            split(shape * 3.9999)
        };
        if output == OutputMode::Ratio {
            let index = self.picker.pick(params.shift);
            self.ramp.set_next_ratio(set(range == Range::Audio, index));
        }

        let step = shift / (CHANNELS - 1) as f32;
        let partial_step = shift / CHANNELS as f32;
        let increment = if shift > 0.0 { 1.0 - pw } else { pw } * step;
        let per_channel: [f32; CHANNELS] = std::array::from_fn(|j| pw + increment * j as f32);
        let widths = if output == OutputMode::Phase && mode == RampMode::Ar {
            per_channel
        } else {
            [pw; CHANNELS]
        };
        self.ramp
            .step(mode, output, range, frequency, &widths, drive);

        let fold_out = |unipolar: f32| fold_scaled(tables, mode, unipolar, fold);
        let mut out = [0.0; CHANNELS];
        match output {
            OutputMode::Envelope => {
                let (phase, f) = (self.ramp.phase(0), self.ramp.frequency(0));
                let raw = self.shapers[0].slope(mode, range, phase, 0.0, f, pw);
                let shaped = self.waveshapers[0].shape(tables, mode, phase, raw, curve, blend);
                out[0] = fold_out(shaped) * shift;
                let plain = if is_phasor {
                    self.waveshapers[1].shape(tables, mode, phase, raw, LINEAR_CONTROL_CURVE, 0.0)
                } else {
                    raw
                };
                out[1] = scale(mode, plain);
                out[2] = self.shapers[2].end_of_attack(mode, range, phase, f, pw);
                out[3] = self.shapers[3].end_of_cycle(mode, range, phase, f);
            }
            OutputMode::Amplitude => {
                let (phase, f) = (self.ramp.phase(0), self.ramp.frequency(0));
                let raw = self.shapers[0].slope(mode, range, phase, 0.0, f, pw);
                let shaped = self.waveshapers[0].shape(tables, mode, phase, raw, curve, blend);
                let sign = if shift < 0.0 { -1.0 } else { 1.0 };
                let value = fold_out(shaped) * sign;
                // Past the last output, it stays there at full level.
                let position = (shift * 5.1).abs().min(CHANNELS as f32);
                for (j, slot) in out.iter_mut().enumerate() {
                    let gain = (1.0 - ((j + 1) as f32 - position).abs()).max(0.0);
                    // Equal power at audio rate, where outputs get mixed.
                    let power = if range == Range::Audio {
                        2.0 - gain
                    } else {
                        1.0
                    };
                    *slot = value * gain * power;
                }
            }
            OutputMode::Phase => {
                let mut phase_shift = 0.0;
                for (j, slot) in out.iter_mut().enumerate() {
                    let source = if mode == RampMode::Ar { j } else { 0 };
                    let width = if mode == RampMode::Ad {
                        per_channel[j]
                    } else {
                        pw
                    };
                    let phase = self.ramp.phase(source);
                    let raw = self.shapers[j].slope(
                        mode,
                        range,
                        phase,
                        phase_shift,
                        self.ramp.frequency(source),
                        width,
                    );
                    let shaped = self.waveshapers[j].shape(tables, mode, phase, raw, curve, blend);
                    *slot = fold_out(shaped);
                    phase_shift -= if range == Range::Audio {
                        step
                    } else {
                        partial_step
                    };
                }
            }
            OutputMode::Ratio => {
                for (j, slot) in out.iter_mut().enumerate() {
                    let (phase, f) = (self.ramp.phase(j), self.ramp.frequency(j));
                    let raw = self.shapers[j].slope(mode, range, phase, 0.0, f, pw);
                    let shaped = self.waveshapers[j].shape(tables, mode, phase, raw, curve, blend);
                    *slot = fold_out(shaped);
                }
            }
        }

        if smoothness < 0.5 {
            let amount = (smoothness * 2.0).powi(4);
            let smoothed = if output == OutputMode::Envelope {
                1
            } else {
                CHANNELS
            };
            for (j, value) in out.iter_mut().enumerate().take(smoothed) {
                let source = if output == OutputMode::Ratio { j } else { 0 };
                let mut coefficient = self.ramp.frequency(source) * 0.5;
                coefficient += (1.0 - coefficient) * amount;
                let [one, two] = &mut self.smooth[j];
                *one += coefficient * (*value - *one);
                *two += coefficient * (*one - *two);
                *value = *two;
            }
        } else {
            // Follow the output, so smoothing starts from where it is.
            for (state, value) in self.smooth.iter_mut().zip(out) {
                *state = [value; 2];
            }
        }
        out.map(|v| v.clamp(-1.0, 1.0))
    }
}

/// How much of a shape or fold to keep at frequency `f0`, given how many
/// `harmonics` it adds and the filter `order` it is allowed: all of it
/// well below Nyquist, none of it near.
#[inline]
fn tame(f0: f32, harmonics: f32, order: f32) -> f32 {
    let f0 = f0 * harmonics;
    let max_f = 0.5 / order;
    let amount = (1.0 - (f0 - max_f) / (0.5 - max_f)).clamp(0.0, 1.0);
    amount * amount * amount
}

/// A curve position as the curve and how far toward the next.
#[inline]
fn split(position: f32) -> (usize, f32) {
    let whole = position.max(0.0) as usize;
    (whole, position - whole as f32)
}

/// A shaped unipolar slope folded and scaled for its mode: a cycle
/// becomes bipolar, an envelope stays 0..1.
#[inline]
fn fold_scaled(tables: &Tables, mode: RampMode, unipolar: f32, amount: f32) -> f32 {
    if mode == RampMode::Looping {
        tables.fold_bipolar(2.0 * unipolar - 1.0, amount)
    } else {
        tables.fold_unipolar(unipolar, amount).clamp(0.0, 1.0)
    }
}

/// An unshaped slope scaled for its mode.
#[inline]
fn scale(mode: RampMode, unipolar: f32) -> f32 {
    if mode == RampMode::Looping {
        2.0 * unipolar - 1.0
    } else {
        unipolar
    }
}

#[cfg(test)]
#[path = "generator_tests.rs"]
mod tests;
