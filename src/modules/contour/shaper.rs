//! Ported from Mutable Instruments' `tides2/ramp_shaper.h` (MIT, Copyright
//! 2017 Emilie Gillet), adapted to Fugue: outputs are 0..1, and the
//! band-limited corners and edges use the four-sample B-spline residuals
//! from [`crate::dsp`]. See THIRD_PARTY_NOTICES.md.
//!
//! A [`RampShaper`] turns a phase into the contour's raw slope: an attack
//! and a decay whose split is set by `slope`, or the end-of-attack and
//! end-of-cycle gates. A [`Waveshaper`] bends that slope through the
//! curves `shape` picks.

use super::tables::Tables;
use super::{RampMode, Range};
use crate::dsp::{corner_residual, step_residual};

/// Turns a phase into a slope or a gate. One per output, since a
/// band-limited output holds samples back for later corrections.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RampShaper {
    /// The two samples held back, oldest first, still open to corrections.
    held: [f32; 2],
    /// A correction waiting for the next sample.
    ahead: f32,
    previous_shift: f32,
    /// Whether `previous_shift` holds a shift seen, so the first one is
    /// not taken for a move.
    shifting: bool,
}

/// Corrections for the samples two and one back, the new one and the next.
type Fix = [f32; 4];

impl RampShaper {
    /// The slope at `phase` (0..1), unipolar 0..1. AD and control-rate
    /// cycles give an attack over 0..0.5 and a decay over 0..5..1, for the
    /// waveshaper's attack-decay curves; AR passes the phase on; an
    /// audio-rate cycle is a band-limited rise and fall.
    #[inline]
    pub(super) fn slope(
        &mut self,
        mode: RampMode,
        range: Range,
        phase: f32,
        shift: f32,
        frequency: f32,
        pw: f32,
    ) -> f32 {
        match (mode, range) {
            (RampMode::Ad, _) => self.skewed(phase, 0.0, frequency, pw),
            (RampMode::Ar, _) => phase,
            (RampMode::Looping, Range::Control) => self.skewed(phase, shift, frequency, pw),
            (RampMode::Looping, Range::Audio) => {
                self.band_limited_slope(phase, shift, frequency, pw)
            }
        }
    }

    /// The end-of-attack gate: 1 from the top of the attack to the end of
    /// the cycle.
    #[inline]
    pub(super) fn end_of_attack(
        &mut self,
        mode: RampMode,
        range: Range,
        phase: f32,
        frequency: f32,
        pw: f32,
    ) -> f32 {
        // An envelope at rest (phase 1) has no attack to mark.
        match (mode, range) {
            (RampMode::Looping, Range::Audio) => self.band_limited_pulse(phase, frequency, pw),
            (RampMode::Looping, Range::Control) => gate(phase >= constrain_width(pw, frequency)),
            (RampMode::Ar, _) => gate((0.5..1.0).contains(&phase)),
            (RampMode::Ad, _) => gate(phase >= constrain_width(pw, frequency) && phase < 1.0),
        }
    }

    /// The end-of-cycle gate: a short pulse as a cycle starts, or high once
    /// a one-shot has finished.
    #[inline]
    pub(super) fn end_of_cycle(
        &mut self,
        mode: RampMode,
        range: Range,
        phase: f32,
        frequency: f32,
    ) -> f32 {
        match (mode, range) {
            (RampMode::Looping, Range::Audio) => {
                let width = (96.0 * frequency).min(0.5);
                1.0 - self.band_limited_pulse(phase, frequency, width)
            }
            (RampMode::Looping, Range::Control) => gate(phase < (96.0 * frequency).min(0.5)),
            _ => gate(phase >= 1.0),
        }
    }

    /// `phase` and `frequency` moved by a phase offset that may itself be
    /// moving.
    #[inline]
    fn shifted(&mut self, phase: f32, shift: f32, frequency: f32) -> (f32, f32) {
        let moved = if self.shifting {
            shift - self.previous_shift
        } else {
            0.0
        };
        self.shifting = true;
        self.previous_shift = shift;
        if shift == 0.0 && moved == 0.0 {
            return (phase, frequency);
        }
        let phase = phase + shift;
        (phase - phase.floor(), frequency + moved)
    }

    /// A rise over the first `pw` of the cycle and a fall over the rest,
    /// as 0..0.5 then 0.5..1.
    #[inline]
    fn skewed(&mut self, phase: f32, shift: f32, frequency: f32, pw: f32) -> f32 {
        let (phase, frequency) = self.shifted(phase, shift, frequency);
        let pw = constrain_width(pw, frequency);
        if phase < pw {
            phase * 0.5 / pw
        } else {
            (phase - pw) * 0.5 / (1.0 - pw) + 0.5
        }
    }

    /// A rise from 0 to 1 over the first `pw` of the cycle and a fall back
    /// over the rest, with both corners band-limited. Two samples late.
    #[inline]
    fn band_limited_slope(&mut self, phase: f32, shift: f32, frequency: f32, pw: f32) -> f32 {
        let (phase, frequency) = self.shifted(phase, shift, frequency);
        let pw = constrain_width(pw, frequency);
        let up = 1.0 / pw;
        let down = 1.0 / (1.0 - pw);
        let mut fix = Fix::default();
        if let Some((since, at_width)) = crossing(phase, frequency, pw) {
            // The slope turns from rising to falling at the width, and
            // back at the start of the cycle.
            let turn = (up + down) * frequency;
            add(
                &mut fix,
                since,
                if at_width { -turn } else { turn },
                corner_residual,
            );
        }
        let value = if phase < pw {
            phase * up
        } else {
            1.0 - (phase - pw) * down
        };
        self.emit(fix, value)
    }

    /// A pulse, 0 for the first `pw` of the cycle and 1 for the rest, with
    /// both edges band-limited. Two samples late.
    #[inline]
    fn band_limited_pulse(&mut self, phase: f32, frequency: f32, pw: f32) -> f32 {
        let pw = constrain_width(pw, frequency);
        let mut fix = Fix::default();
        if let Some((since, at_width)) = crossing(phase, frequency, pw) {
            add(
                &mut fix,
                since,
                if at_width { 1.0 } else { -1.0 },
                step_residual,
            );
        }
        self.emit(fix, gate(phase >= pw))
    }

    /// Releases the oldest held sample with its corrections and holds the
    /// new one.
    #[inline]
    fn emit(&mut self, fix: Fix, value: f32) -> f32 {
        let out = self.held[0] + fix[0];
        self.held = [self.held[1] + fix[1], value + self.ahead + fix[2]];
        self.ahead = fix[3];
        out
    }
}

#[inline]
fn gate(high: bool) -> f32 {
    if high {
        1.0
    } else {
        0.0
    }
}

/// The shortest share of a cycle a rise or a fall may take, so that a
/// stopped ramp never divides by zero.
const MIN_WIDTH: f32 = 1.0e-6;

/// Keeps the rise and the fall at least two samples long.
#[inline]
fn constrain_width(pw: f32, frequency: f32) -> f32 {
    let edge = (2.0 * frequency.abs()).clamp(MIN_WIDTH, 0.5);
    pw.max(edge).min(1.0 - edge)
}

/// If the phase passed the width or the start of the cycle during the
/// last sample: how long ago, in samples, and whether it was the width.
#[inline]
fn crossing(phase: f32, frequency: f32, pw: f32) -> Option<(f32, bool)> {
    let (point, at_width) = if phase < pw * 0.5 {
        (0.0, false)
    } else {
        (pw, true)
    };
    let d = phase - point;
    (d >= 0.0 && d < frequency).then(|| (d / frequency, at_width))
}

/// Adds `height` of a residual `since` samples after its event to the
/// four samples open to correction.
#[inline]
fn add(fix: &mut Fix, since: f32, height: f32, residual: fn(f32) -> f32) {
    for (k, slot) in fix.iter_mut().enumerate() {
        *slot += height * residual(since + k as f32 - 2.0);
    }
}

/// Bends a slope through the curves `shape` picks. In AR and AD it also
/// keeps the envelope continuous when a gate or trigger arrives mid-way:
/// the new stage starts from where the envelope is, whichever stage it
/// interrupts.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Waveshaper {
    previous_input: f32,
    previous_phase: f32,
    previous_output: f32,
    breakpoint: f32,
}

impl Waveshaper {
    /// `input` (0..1), the slope at the ramp's `phase`, through `curve`,
    /// blended `blend` toward the next.
    #[inline]
    pub(super) fn shape(
        &mut self,
        tables: &Tables,
        mode: RampMode,
        phase: f32,
        input: f32,
        curve: usize,
        blend: f32,
    ) -> f32 {
        let output = tables.shape(input, curve, blend);
        if mode == RampMode::Looping {
            return output;
        }
        // Entering a stage, start it from the level reached: the decay
        // from the top of the attack, a retrigger (the ramp starting
        // again) from wherever the envelope was.
        let (before, now) = (self.previous_input, input);
        let restarted = phase < self.previous_phase;
        self.previous_phase = phase;
        if (before <= 0.5 && now > 0.5) || restarted {
            self.breakpoint = self.previous_output;
        } else if now == 1.0 {
            self.breakpoint = 1.0;
        } else if now == 0.5 {
            self.breakpoint = 0.0;
        }
        let output = if now <= 0.5 {
            self.breakpoint + (1.0 - self.breakpoint) * output
        } else {
            self.breakpoint * output
        };
        self.previous_input = now;
        self.previous_output = output;
        output
    }
}

#[cfg(test)]
mod tests {
    use super::super::tables::tables;
    use super::*;

    #[test]
    fn a_skewed_ramp_rises_for_the_width_then_falls() {
        let mut shaper = RampShaper::default();
        let at = |s: &mut RampShaper, p| s.slope(RampMode::Ad, Range::Control, p, 0.0, 0.001, 0.25);
        assert_eq!(at(&mut shaper, 0.0), 0.0);
        assert!((at(&mut shaper, 0.125) - 0.25).abs() < 1e-6);
        assert!((at(&mut shaper, 0.25) - 0.5).abs() < 1e-6);
        assert!((at(&mut shaper, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_band_limited_slope_matches_the_naive_one_away_from_corners() {
        let mut shaper = RampShaper::default();
        let f = 0.01;
        let mut phase = 0.0f32;
        let mut naive = Vec::new();
        let mut limited = Vec::new();
        for _ in 0..400 {
            phase += f;
            phase -= phase.floor();
            limited.push(shaper.slope(RampMode::Looping, Range::Audio, phase, 0.0, f, 0.3));
            naive.push(if phase < 0.3 {
                phase / 0.3
            } else {
                1.0 - (phase - 0.3) / 0.7
            });
        }
        // Two samples late; equal except within two samples of a corner.
        for i in 10..398 {
            let p = (i as f32 + 1.0) * f % 1.0;
            let near = |c: f32| ((p - c + 1.5) % 1.0 - 0.5).abs() < 3.0 * f;
            if !near(0.0) && !near(0.3) {
                assert!((limited[i + 2] - naive[i]).abs() < 1e-3, "{i}");
            }
        }
    }

    #[test]
    fn a_retrigger_starts_the_attack_from_the_current_level() {
        let t = tables();
        let mut shaper = Waveshaper::default();
        let curve = super::super::tables::AUDIO_CURVES + 3;
        // Through the attack and part of the decay.
        let mut level = 0.0;
        for i in 0..=75 {
            level = shaper.shape(
                t,
                RampMode::Ad,
                i as f32 / 100.0,
                i as f32 / 100.0,
                curve,
                0.0,
            );
        }
        assert!(level > 0.4 && level < 0.6);
        // Retriggered: the attack starts from that level, not from 0.
        let next = shaper.shape(t, RampMode::Ad, 0.01, 0.01, curve, 0.0);
        assert!((next - level).abs() < 0.02, "{level} -> {next}");
    }

    #[test]
    fn a_retrigger_during_the_attack_carries_on_from_the_current_level() {
        let t = tables();
        let mut shaper = Waveshaper::default();
        let curve = super::super::tables::AUDIO_CURVES + 3;
        let mut level = 0.0;
        for i in 0..=30 {
            level = shaper.shape(
                t,
                RampMode::Ad,
                i as f32 / 100.0,
                i as f32 / 100.0,
                curve,
                0.0,
            );
        }
        assert!(level > 0.4, "most of the way up the attack, {level}");
        let next = shaper.shape(t, RampMode::Ad, 0.0, 0.0, curve, 0.0);
        assert!((next - level).abs() < 0.02, "{level} -> {next}");
        // And it still reaches the top.
        let top = (0..=50)
            .map(|i| {
                shaper.shape(
                    t,
                    RampMode::Ad,
                    i as f32 / 100.0,
                    i as f32 / 100.0,
                    curve,
                    0.0,
                )
            })
            .fold(0.0, f32::max);
        assert!(top > 0.99, "{top}");
    }
}
