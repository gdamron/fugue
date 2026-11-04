//! Ported from Mutable Instruments' `plaits/dsp/oscillator/grainlet_oscillator.h`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block
//! processing. See THIRD_PARTY_NOTICES.md.

use crate::dsp::{sine, BlockRamp};
use crate::modules::macro_oscillator::engine::{next_blep_sample, this_blep_sample, MAX_FREQUENCY};

/// A train of grains: each cycle of the carrier windows a sine at the
/// formant frequency, restarted at the cycle's start, so the formant
/// rings as a fixed resonance over any pitch. The restart is smoothed
/// with a polyBLEP.
pub(in crate::modules::macro_oscillator) struct GrainletOscillator {
    carrier_phase: f32,
    formant_phase: f32,
    next_sample: f32,
    carrier_frequency: BlockRamp,
    formant_frequency: BlockRamp,
    carrier_shape: BlockRamp,
    carrier_bleed: BlockRamp,
}

impl GrainletOscillator {
    pub fn new() -> Self {
        Self {
            carrier_phase: 0.0,
            formant_phase: 0.0,
            next_sample: 0.0,
            carrier_frequency: BlockRamp::new(0.0),
            formant_frequency: BlockRamp::new(0.0),
            carrier_shape: BlockRamp::new(0.0),
            carrier_bleed: BlockRamp::new(0.0),
        }
    }

    /// Renders grains at `carrier_frequency` filled with a sine at
    /// `formant_frequency` (both cycles per sample). `carrier_shape` sets
    /// the window, from a short burst through a smooth bell to a slow
    /// swell; `carrier_bleed` mixes the bare window in.
    pub fn render(
        &mut self,
        carrier_frequency: f32,
        formant_frequency: f32,
        carrier_shape: f32,
        carrier_bleed: f32,
        out: &mut [f32],
    ) {
        let size = out.len();
        let carrier_frequency = carrier_frequency.min(MAX_FREQUENCY * 0.5);
        let formant_frequency = formant_frequency.min(MAX_FREQUENCY);
        // Each ramp's value at the previous frame, for reads between frames.
        let mut last_shape = self.carrier_shape.value();
        let mut last_bleed = self.carrier_bleed.value();
        let carriers = self.carrier_frequency.steps(carrier_frequency, size);
        let formants = self.formant_frequency.steps(formant_frequency, size);
        let shapes = self.carrier_shape.steps(carrier_shape, size);
        let bleeds = self.carrier_bleed.steps(carrier_bleed, size);
        let mut next_sample = self.next_sample;

        for (o, ((f0, f1), (shape, bleed))) in out
            .iter_mut()
            .zip(carriers.zip(formants).zip(shapes.zip(bleeds)))
        {
            let mut this_sample = next_sample;
            next_sample = 0.0;
            self.carrier_phase += f0;
            if self.carrier_phase >= 1.0 {
                self.carrier_phase -= 1.0;
                let reset_time = self.carrier_phase / f0;
                let between = 1.0 - reset_time;
                let before = grainlet(
                    1.0,
                    self.formant_phase + between * f1,
                    last_shape + (shape - last_shape) * between,
                    last_bleed + (bleed - last_bleed) * between,
                );
                let after = grainlet(0.0, 0.0, shape, bleed);
                let discontinuity = after - before;
                this_sample += discontinuity * this_blep_sample(reset_time);
                next_sample += discontinuity * next_blep_sample(reset_time);
                self.formant_phase = reset_time * f1;
            } else {
                self.formant_phase += f1;
                if self.formant_phase >= 1.0 {
                    self.formant_phase -= 1.0;
                }
            }
            next_sample += grainlet(self.carrier_phase, self.formant_phase, shape, bleed);
            *o = this_sample;
            last_shape = shape;
            last_bleed = bleed;
        }
        self.next_sample = next_sample;
    }
}

/// The grain window at `phase` of the carrier cycle, 0 to 1: `shape` 0
/// is a short burst, 1/3 a bell bent toward the start, 2/3 a symmetric
/// bell, 1 a slow swell.
#[inline]
fn carrier(phase: f32, shape: f32) -> f32 {
    let shape = shape * 3.0;
    let integral = (shape as usize).min(3);
    let fractional = shape - integral as f32;
    let t = 1.0 - fractional;
    let phase = match integral {
        0 => (phase * (1.0 + t * t * t * 15.0)).min(1.0) + 0.75,
        1 => {
            let breakpoint = 0.001 + 0.499 * t * t * t;
            let phase = if phase < breakpoint {
                phase * (0.5 / breakpoint)
            } else {
                0.5 + (phase - breakpoint) * 0.5 / (1.0 - breakpoint)
            };
            phase + 0.75
        }
        _ => {
            let t = 1.0 - t;
            (0.25 + phase * (0.5 + t * t * t * 14.5)).min(0.75)
        }
    };
    (sine(phase) + 1.0) * 0.25
}

#[inline]
fn grainlet(carrier_phase: f32, formant_phase: f32, shape: f32, bleed: f32) -> f32 {
    let window = carrier(carrier_phase, shape);
    window * (sine(formant_phase) + bleed) / (1.0 + bleed)
}
