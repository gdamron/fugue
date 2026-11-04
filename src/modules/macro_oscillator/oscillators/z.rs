//! Ported from Mutable Instruments' `plaits/dsp/oscillator/z_oscillator.h`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block
//! processing. See THIRD_PARTY_NOTICES.md.

use crate::dsp::{sine, BlockRamp};
use crate::modules::macro_oscillator::engine::{next_blep_sample, this_blep_sample, MAX_FREQUENCY};

/// A sine at the formant frequency, restarted twice per carrier cycle and
/// faded out between restarts, then shaped by the carrier: the sound of a
/// sync-swept resonant filter, like phase-distortion resonant waveforms.
pub(in crate::modules::macro_oscillator) struct ZOscillator {
    carrier_phase: f32,
    /// Runs at twice the carrier rate: each wrap restarts the formant.
    discontinuity_phase: f32,
    formant_phase: f32,
    next_sample: f32,
    carrier_frequency: BlockRamp,
    formant_frequency: BlockRamp,
    carrier_shape: BlockRamp,
    mode: BlockRamp,
}

impl ZOscillator {
    pub fn new() -> Self {
        Self {
            carrier_phase: 0.0,
            discontinuity_phase: 0.0,
            formant_phase: 0.0,
            next_sample: 0.0,
            carrier_frequency: BlockRamp::new(0.0),
            formant_frequency: BlockRamp::new(0.0),
            carrier_shape: BlockRamp::new(0.0),
            mode: BlockRamp::new(0.0),
        }
    }

    /// Renders at `carrier_frequency` with the resonance at
    /// `formant_frequency` (both cycles per sample). `carrier_shape` and
    /// `mode` (both 0 to 1) pick the envelope over the cycle and the
    /// resonance's phase and offset.
    pub fn render(
        &mut self,
        carrier_frequency: f32,
        formant_frequency: f32,
        carrier_shape: f32,
        mode: f32,
        out: &mut [f32],
    ) {
        let size = out.len();
        let carrier_frequency = carrier_frequency.min(MAX_FREQUENCY * 0.5);
        let formant_frequency = formant_frequency.min(MAX_FREQUENCY);
        let mut last_shape = self.carrier_shape.value();
        let mut last_mode = self.mode.value();
        let carriers = self.carrier_frequency.steps(carrier_frequency, size);
        let formants = self.formant_frequency.steps(formant_frequency, size);
        let shapes = self.carrier_shape.steps(carrier_shape, size);
        let modes = self.mode.steps(mode, size);
        let mut next_sample = self.next_sample;

        for (o, ((f0, f1), (shape, mode))) in out
            .iter_mut()
            .zip(carriers.zip(formants).zip(shapes.zip(modes)))
        {
            let mut this_sample = next_sample;
            next_sample = 0.0;
            self.discontinuity_phase += 2.0 * f0;
            self.carrier_phase += f0;
            if self.discontinuity_phase >= 1.0 {
                self.discontinuity_phase -= 1.0;
                let reset_time = self.discontinuity_phase / (2.0 * f0);
                let between = 1.0 - reset_time;
                let wrapped = self.carrier_phase >= 1.0;
                let before = z(
                    if wrapped { 1.0 } else { 0.5 },
                    1.0,
                    self.formant_phase + between * f1,
                    last_shape + (shape - last_shape) * between,
                    last_mode + (mode - last_mode) * between,
                );
                let after = z(if wrapped { 0.0 } else { 0.5 }, 0.0, 0.0, shape, mode);
                let discontinuity = after - before;
                this_sample += discontinuity * this_blep_sample(reset_time);
                next_sample += discontinuity * next_blep_sample(reset_time);
                self.formant_phase = reset_time * f1;
                if self.carrier_phase > 1.0 {
                    self.carrier_phase = self.discontinuity_phase * 0.5;
                }
            } else {
                self.formant_phase += f1;
                if self.formant_phase >= 1.0 {
                    self.formant_phase -= 1.0;
                }
            }
            if self.carrier_phase >= 1.0 {
                self.carrier_phase -= 1.0;
            }
            next_sample += z(
                self.carrier_phase,
                self.discontinuity_phase,
                self.formant_phase,
                shape,
                mode,
            );
            *o = this_sample;
            last_shape = shape;
            last_mode = mode;
        }
        self.next_sample = next_sample;
    }
}

/// One sample: carrier phase `c`, restart phase `d`, formant phase `f`.
#[inline]
fn z(c: f32, d: f32, f: f32, shape: f32, mode: f32) -> f32 {
    let mut ramp_down = 0.5 * (1.0 + sine(0.5 * d + 0.25));
    let (offset, phase_shift) = if mode < 0.333 {
        (1.0, 0.25 + mode * 1.50)
    } else if mode < 0.666 {
        let phase_shift = 0.7495 - (mode - 0.33) * 0.75;
        (-sine(phase_shift), phase_shift)
    } else {
        (0.001, 0.7495 - (mode - 0.33) * 0.75)
    };
    let discontinuity = sine(f + phase_shift);
    let contour = if shape < 0.5 {
        let shape = shape * 2.0;
        if c >= 0.5 {
            ramp_down *= shape;
        }
        1.0 + (sine(c + 0.25) - 1.0) * shape
    } else {
        sine(c + shape * 0.5)
    };
    (ramp_down * (offset + discontinuity) - offset) * contour
}
