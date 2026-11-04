//! Ported from Mutable Instruments' `plaits/dsp/envelope.h` and
//! `plaits/dsp/fx/low_pass_gate.h` (MIT, Copyright 2014-2016 Emilie Gillet),
//! adapted to Fugue's block processing and its state-variable filter.
//! See THIRD_PARTY_NOTICES.md.
//!
//! A low-pass gate: a level and a lowpass cutoff moved together by one
//! slow, vactrol-like envelope, so a note darkens as it fades the way a
//! plucked or struck one does.

use crate::dsp::{BlockRamp, Svf};

/// The gate filter's resonance: gentle, with no peak.
const GATE_Q: f32 = 0.4;

/// The rate the envelope's coefficients were tuned at, per block of 12.
pub(super) const TUNED_RATE: f32 = 48_000.0;

/// The vactrol-like envelope behind the gate: how open it is, and the
/// level, cutoff and high-frequency bleed that follow from that.
pub(super) struct LpgEnvelope {
    vactrol_state: f32,
    gain: f32,
    /// Cutoff as a fraction of [`TUNED_RATE`].
    frequency: f32,
    hf_bleed: f32,
    ramp_up: bool,
}

impl LpgEnvelope {
    pub fn new() -> Self {
        Self {
            vactrol_state: 0.0,
            gain: 1.0,
            frequency: 0.5,
            hf_bleed: 0.0,
            ramp_up: false,
        }
    }

    /// Closes the gate and forgets any ping in progress.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Starts a ping: the gate opens at `attack` per block, then decays.
    pub fn trigger(&mut self) {
        self.ramp_up = true;
    }

    /// One block of a pinged gate. Coefficients are per block of 12 frames
    /// at [`TUNED_RATE`]; the caller scales them for other block lengths.
    pub fn process_ping(
        &mut self,
        attack: f32,
        short_decay: f32,
        decay_tail: f32,
        hf: f32,
        rise: f32,
    ) {
        if self.ramp_up {
            self.vactrol_state += attack;
            if self.vactrol_state >= 1.0 {
                self.vactrol_state = 1.0;
                self.ramp_up = false;
            }
        }
        let level = if self.ramp_up {
            self.vactrol_state
        } else {
            0.0
        };
        self.process_level(level, short_decay, decay_tail, hf, rise);
    }

    /// One block of a gate following `level` (0 to 1). `rise` is the
    /// coefficient for an opening gate (0.6 at the tuned block length).
    pub fn process_level(
        &mut self,
        level: f32,
        short_decay: f32,
        decay_tail: f32,
        hf: f32,
        rise: f32,
    ) {
        let error = level - self.vactrol_state;
        let state_2 = self.vactrol_state * self.vactrol_state;
        let state_4 = state_2 * state_2;
        let tail = 1.0 - self.vactrol_state;
        let tail_2 = tail * tail;
        let coefficient = if error > 0.0 {
            rise
        } else {
            short_decay + (1.0 - state_4) * decay_tail
        };
        self.vactrol_state += coefficient * error;

        self.gain = self.vactrol_state;
        self.frequency = 0.003 + 0.3 * state_4 + hf * 0.04;
        self.hf_bleed = (tail_2 + (1.0 - tail_2) * hf) * hf * hf;
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// The cutoff in Hz.
    pub fn cutoff(&self) -> f32 {
        self.frequency * TUNED_RATE
    }

    pub fn hf_bleed(&self) -> f32 {
        self.hf_bleed
    }
}

/// The gate itself: a level and a lowpass filter, with some of the
/// unfiltered signal bled through.
pub(super) struct LowPassGate {
    gain: BlockRamp,
    filter: Svf,
}

impl LowPassGate {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            gain: BlockRamp::new(0.0),
            filter: Svf::new(sample_rate, 1_000.0, GATE_Q),
        }
    }

    pub fn reset(&mut self) {
        self.gain.snap(0.0);
        self.filter.reset();
    }

    /// Gates `block` in place, gliding from the last block's gain to `gain`.
    pub fn process(&mut self, gain: f32, cutoff: f32, hf_bleed: f32, block: &mut [f32]) {
        let gain_ramp = self.gain.steps(gain, block.len());
        self.filter.set(cutoff, GATE_Q);
        for (sample, gain) in block.iter_mut().zip(gain_ramp) {
            let s = *sample * gain;
            let lp = self.filter.process(s).lowpass;
            *sample = lp + (s - lp) * hf_bleed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_gate_is_silent() {
        let mut envelope = LpgEnvelope::new();
        let mut gate = LowPassGate::new(48_000);
        for _ in 0..100 {
            envelope.process_level(0.0, 0.05, 0.01, 0.5, 0.6);
            let mut block = [1.0; 12];
            gate.process(
                envelope.gain(),
                envelope.cutoff(),
                envelope.hf_bleed(),
                &mut block,
            );
            assert!(block.iter().all(|s| *s == 0.0), "{block:?}");
        }
    }

    #[test]
    fn a_ping_opens_then_decays() {
        let mut envelope = LpgEnvelope::new();
        envelope.trigger();
        let mut peak = 0.0f32;
        for _ in 0..10 {
            envelope.process_ping(0.5, 0.05, 0.01, 0.5, 0.6);
            peak = peak.max(envelope.gain());
        }
        assert!(peak > 0.9, "opens to {peak}");
        for _ in 0..2_000 {
            envelope.process_ping(0.5, 0.05, 0.01, 0.5, 0.6);
        }
        assert!(envelope.gain() < 1e-3, "decays to {}", envelope.gain());
    }
}
