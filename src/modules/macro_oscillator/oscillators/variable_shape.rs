//! Ported from Mutable Instruments' `plaits/dsp/oscillator/variable_shape_oscillator.h`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block processing.
//! See THIRD_PARTY_NOTICES.md.
//!
//! A continuously variable waveform, triangle through saw to square, with
//! pulse width and optional hard sync to a master oscillator. Every
//! discontinuity, in value or in slope, is smoothed with a polyBLEP.

use crate::dsp::BlockRamp;
use crate::modules::macro_oscillator::engine::{
    next_blep_sample, next_integrated_blep_sample, this_blep_sample, this_integrated_blep_sample,
    MAX_FREQUENCY,
};

pub(in crate::modules::macro_oscillator) struct VariableShapeOscillator {
    master_phase: f32,
    slave_phase: f32,
    next_sample: f32,
    previous_pw: f32,
    high: bool,

    master_frequency: BlockRamp,
    slave_frequency: BlockRamp,
    pw: BlockRamp,
    waveshape: BlockRamp,
}

impl VariableShapeOscillator {
    pub fn new() -> Self {
        Self {
            master_phase: 0.0,
            slave_phase: 0.0,
            next_sample: 0.0,
            previous_pw: 0.5,
            high: false,
            master_frequency: BlockRamp::new(0.0),
            slave_frequency: BlockRamp::new(0.01),
            pw: BlockRamp::new(0.5),
            waveshape: BlockRamp::new(0.0),
        }
    }

    /// Starts the master oscillator at `phase`, so two synced oscillators
    /// built together don't reset in step.
    pub fn with_master_phase(mut self, phase: f32) -> Self {
        self.master_phase = phase;
        self
    }

    /// Renders `out.len()` samples at `frequency` (cycles per sample).
    /// `waveshape` runs triangle (0), saw (0.5) to square (1). With
    /// `sync`, a master oscillator at `master_frequency` resets the phase.
    pub fn render(
        &mut self,
        sync: bool,
        master_frequency: f32,
        frequency: f32,
        pw: f32,
        waveshape: f32,
        out: &mut [f32],
    ) {
        let size = out.len();
        let master_frequency = master_frequency.min(MAX_FREQUENCY);
        let frequency = frequency.min(MAX_FREQUENCY);
        let pw = if frequency >= 0.25 {
            0.5
        } else {
            pw.clamp(frequency * 2.0, 1.0 - 2.0 * frequency)
        };

        let mut master_fm = self.master_frequency.steps(master_frequency, size);
        let mut fm = self.slave_frequency.steps(frequency, size);
        let mut pwm = self.pw.steps(pw, size);
        let mut waveshape_modulation = self.waveshape.steps(waveshape, size);

        let mut next_sample = self.next_sample;
        for sample in out.iter_mut() {
            let mut reset = false;
            let mut transition_during_reset = false;
            let mut reset_time = 0.0;

            let mut this_sample = next_sample;
            next_sample = 0.0;

            let master_frequency = master_fm.next().unwrap_or(master_frequency);
            let slave_frequency = fm.next().unwrap_or(frequency);
            let pw = pwm.next().unwrap_or(pw);
            let waveshape = waveshape_modulation.next().unwrap_or(waveshape);

            let square_amount = (waveshape - 0.5).max(0.0) * 2.0;
            let triangle_amount = (1.0 - waveshape * 2.0).max(0.0);
            let slope_up = 1.0 / pw;
            let slope_down = 1.0 / (1.0 - pw);

            if sync {
                self.master_phase += master_frequency;
                if self.master_phase >= 1.0 {
                    self.master_phase -= 1.0;
                    reset_time = self.master_phase / master_frequency;
                    let mut slave_phase_at_reset =
                        self.slave_phase + (1.0 - reset_time) * slave_frequency;
                    reset = true;
                    if slave_phase_at_reset >= 1.0 {
                        slave_phase_at_reset -= 1.0;
                        transition_during_reset = true;
                    }
                    if !self.high && slave_phase_at_reset >= pw {
                        transition_during_reset = true;
                    }
                    let value = naive_sample(
                        slave_phase_at_reset,
                        pw,
                        slope_up,
                        slope_down,
                        triangle_amount,
                        square_amount,
                    );
                    this_sample -= value * this_blep_sample(reset_time);
                    next_sample -= value * next_blep_sample(reset_time);
                }
            }

            self.slave_phase += slave_frequency;
            // Each edge crossed this sample, until the phase rests short
            // of the next one.
            if transition_during_reset || !reset {
                loop {
                    if !self.high {
                        if self.slave_phase < pw {
                            break;
                        }
                        let t = (self.slave_phase - pw) / (self.previous_pw - pw + slave_frequency);
                        let triangle_step =
                            (slope_up + slope_down) * slave_frequency * triangle_amount;
                        this_sample += square_amount * this_blep_sample(t);
                        next_sample += square_amount * next_blep_sample(t);
                        this_sample -= triangle_step * this_integrated_blep_sample(t);
                        next_sample -= triangle_step * next_integrated_blep_sample(t);
                        self.high = true;
                    }
                    if self.high {
                        if self.slave_phase < 1.0 {
                            break;
                        }
                        self.slave_phase -= 1.0;
                        let t = self.slave_phase / slave_frequency;
                        let triangle_step =
                            (slope_up + slope_down) * slave_frequency * triangle_amount;
                        this_sample -= (1.0 - triangle_amount) * this_blep_sample(t);
                        next_sample -= (1.0 - triangle_amount) * next_blep_sample(t);
                        this_sample += triangle_step * this_integrated_blep_sample(t);
                        next_sample += triangle_step * next_integrated_blep_sample(t);
                        self.high = false;
                    }
                }
            }

            if sync && reset {
                self.slave_phase = reset_time * slave_frequency;
                self.high = false;
            }

            next_sample += naive_sample(
                self.slave_phase,
                pw,
                slope_up,
                slope_down,
                triangle_amount,
                square_amount,
            );
            self.previous_pw = pw;
            *sample = 2.0 * this_sample - 1.0;
        }
        self.next_sample = next_sample;
    }
}

#[inline]
fn naive_sample(
    phase: f32,
    pw: f32,
    slope_up: f32,
    slope_down: f32,
    triangle_amount: f32,
    square_amount: f32,
) -> f32 {
    let mut saw = phase;
    let square = if phase < pw { 0.0 } else { 1.0 };
    let triangle = if phase < pw {
        phase * slope_up
    } else {
        1.0 - (phase - pw) * slope_down
    };
    saw += (square - saw) * square_amount;
    saw += (triangle - saw) * triangle_amount;
    saw
}
