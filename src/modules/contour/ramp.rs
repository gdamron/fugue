//! Ported from Mutable Instruments' `tides2/ramp_generator.h` (MIT,
//! Copyright 2017 Emilie Gillet), adapted to Fugue: per-sample triggers,
//! and a clock that restarts the cycle on every edge. See
//! THIRD_PARTY_NOTICES.md.
//!
//! The [`RampGenerator`] advances one phase per output (or a single one
//! all outputs share) through attack-decay, attack-release or a loop.

use super::ratios::Ratio;
use super::{OutputMode, RampMode, Range};

pub(super) const CHANNELS: usize = 4;

/// The highest frequency a ramp runs at, over the sample rate.
pub(super) const MAX_FREQUENCY: f32 = 0.25;

/// Where a clocked ramp waits for a late edge: just short of the end.
const CLOCK_HOLD: f32 = 1.0 - f32::EPSILON;

/// What drives the ramp this sample.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Drive {
    /// The trigger input rose.
    pub rising: bool,
    /// The trigger input is high (the AR gate).
    pub high: bool,
    /// The clock's phase through its period (0..1, 0 on the edge), when a
    /// clock drives the ramp in place of the trigger.
    pub clock: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RampGenerator {
    /// The ratios each output switches to at its next restart.
    next_ratio: [Ratio; CHANNELS],
    ratio: [Ratio; CHANNELS],
    master_phase: f32,
    /// How many master cycles each output has run since it last restarted
    /// with the master.
    wrap_counter: [u32; CHANNELS],
    phase: [f32; CHANNELS],
    frequency: [f32; CHANNELS],
}

impl Default for RampGenerator {
    fn default() -> Self {
        Self {
            next_ratio: [Ratio::UNISON; CHANNELS],
            ratio: [Ratio::UNISON; CHANNELS],
            master_phase: 0.0,
            wrap_counter: [0; CHANNELS],
            phase: [0.0; CHANNELS],
            frequency: [0.0; CHANNELS],
        }
    }
}

impl RampGenerator {
    /// A one-shot ramp at rest, waiting for its trigger.
    pub(super) fn at_rest() -> Self {
        Self {
            phase: [1.0; CHANNELS],
            ..Self::default()
        }
    }

    #[inline]
    pub(super) fn phase(&self, channel: usize) -> f32 {
        self.phase[channel]
    }

    #[inline]
    pub(super) fn frequency(&self, channel: usize) -> f32 {
        self.frequency[channel]
    }

    /// Carries this ramp's place into a ramp about to run in another
    /// mode: a looping ramp counts from where the first output is.
    pub(super) fn align(&mut self) {
        self.master_phase = self.phase[0].clamp(0.0, CLOCK_HOLD);
    }

    /// The ratios the outputs take from their next restart.
    #[inline]
    pub(super) fn set_next_ratio(&mut self, ratios: [Ratio; CHANNELS]) {
        self.next_ratio = ratios;
    }

    /// Advances the ramp one sample at frequency `f0` (over the sample
    /// rate), with attack fractions `pw` (one per output, or the first for
    /// all).
    #[inline]
    pub(super) fn step(
        &mut self,
        mode: RampMode,
        output: OutputMode,
        range: Range,
        f0: f32,
        pw: &[f32; CHANNELS],
        drive: Drive,
    ) {
        let n = if output == OutputMode::Ratio
            || (output == OutputMode::Phase && mode == RampMode::Ar)
        {
            CHANNELS
        } else {
            1
        };
        match mode {
            RampMode::Ad => self.step_ad(n, f0, drive),
            RampMode::Ar => self.step_ar(n, output, f0, pw, drive),
            RampMode::Looping => {
                if range == Range::Audio && output == OutputMode::Ratio {
                    self.step_free(n, f0, drive);
                } else {
                    self.step_locked(n, f0, drive);
                }
            }
        }
    }

    fn step_ad(&mut self, n: usize, f0: f32, drive: Drive) {
        if let Some(ramp) = drive.clock {
            self.step_clocked_ad(n, f0, ramp);
            return;
        }
        for i in 0..n {
            self.frequency[i] = (f0 * self.next_ratio[i].value()).min(MAX_FREQUENCY);
            self.phase[i] = if drive.rising {
                // The envelope starts on the trigger's own sample, so its
                // stages last exactly their share of the cycle.
                0.0
            } else {
                (self.phase[i] + self.frequency[i]).min(1.0)
            };
        }
    }

    /// A clock plays envelopes locked to its edges: an output at ratio
    /// `p/q` plays `p` envelopes, end to end, over every `q` periods.
    fn step_clocked_ad(&mut self, n: usize, f0: f32, ramp: f32) {
        if ramp < self.master_phase {
            self.wrap(n);
        }
        self.master_phase = ramp;
        for i in 0..n {
            let ratio = self.ratio[i];
            self.frequency[i] = (f0 * ratio.value()).min(MAX_FREQUENCY);
            let total = (ramp + self.wrap_counter[i] as f32) * ratio.value();
            self.phase[i] = (total - total.floor()).min(CLOCK_HOLD);
        }
    }

    fn step_ar(
        &mut self,
        n: usize,
        output: OutputMode,
        f0: f32,
        pw: &[f32; CHANNELS],
        drive: Drive,
    ) {
        for i in 0..n {
            self.frequency[i] = if output == OutputMode::Phase {
                f0
            } else {
                (f0 * self.next_ratio[i].value()).min(MAX_FREQUENCY)
            };
        }
        // A clock holds the gate high for the first half of each period.
        let up = drive.clock.map_or(drive.high, |ramp| ramp < 0.5);
        let clip_at = if up { 0.5 } else { 1.0 };
        for i in 0..n {
            if self.phase[i] < 0.5 && !up {
                self.phase[i] = 0.5;
            } else if self.phase[i] > 0.5 && up {
                self.phase[i] = 0.0;
            }
            let width = if output == OutputMode::Ratio {
                pw[0]
            } else {
                pw[i]
            };
            // Each stage at least two samples long.
            let edge = (2.0 * self.frequency[i]).clamp(1.0e-6, 0.5);
            let width = width.max(edge).min(1.0 - edge);
            let rate = if self.phase[i] < 0.5 {
                0.5 / (1.0e-6 + width)
            } else {
                0.5 / (1.0 + 1.0e-6 - width)
            };
            self.phase[i] = (self.phase[i] + self.frequency[i] * rate).min(clip_at);
        }
    }

    /// Audio-rate ratios run free, so each output can glide on its own.
    fn step_free(&mut self, n: usize, f0: f32, drive: Drive) {
        for i in 0..n {
            self.frequency[i] = (f0 * self.next_ratio[i].value()).min(MAX_FREQUENCY);
        }
        if drive.rising {
            self.phase[..n].fill(0.0);
            return;
        }
        for i in 0..n {
            self.phase[i] += self.frequency[i];
            self.phase[i] -= self.phase[i].floor();
        }
    }

    /// Every output's phase is a multiple of one master phase, so they
    /// stay locked to it, and to the clock when one rules.
    fn step_locked(&mut self, n: usize, f0: f32, drive: Drive) {
        for i in 0..n {
            self.frequency[i] = (f0 * self.ratio[i].value()).min(MAX_FREQUENCY);
        }
        if drive.rising {
            self.master_phase = 0.0;
            self.ratio = self.next_ratio;
            self.wrap_counter = [0; CHANNELS];
        } else if let Some(ramp) = drive.clock {
            if ramp < self.master_phase {
                self.wrap(n);
            }
            self.master_phase = ramp.min(CLOCK_HOLD);
        } else {
            self.master_phase += f0;
            if self.master_phase >= 1.0 {
                self.master_phase -= self.master_phase.floor();
                self.wrap(n);
            }
        }
        for i in 0..n {
            let ratio = self.ratio[i];
            let mult = (self.master_phase + self.wrap_counter[i] as f32) * ratio.value();
            self.phase[i] = mult - mult.floor();
        }
    }

    /// The master started a new cycle: an output whose ratio's cycle is
    /// complete restarts with it, taking its next ratio.
    fn wrap(&mut self, n: usize) {
        for i in 0..n {
            self.wrap_counter[i] += 1;
            if self.wrap_counter[i] >= self.ratio[i].q {
                self.ratio[i] = self.next_ratio[i];
                self.wrap_counter[i] = 0;
            }
        }
    }
}

/// Follows a clock input: the period between its last two rising edges,
/// and how far through the current one it is.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ClockFollower {
    high: bool,
    /// Samples since the last edge, once there has been one.
    since_edge: Option<u32>,
    /// The last measured period in samples, once there are two edges.
    period: Option<u32>,
}

impl ClockFollower {
    /// Reads one clock sample: whether it rose, and the phase through the
    /// period (`None` until a period is known).
    #[inline]
    pub(super) fn step(&mut self, value: f32) -> (bool, Option<f32>) {
        let high = value > 0.5;
        let rose = high && !self.high;
        self.high = high;
        if rose {
            if let Some(since) = self.since_edge {
                self.period = Some(since + 1);
            }
            self.since_edge = Some(0);
        } else if let Some(since) = self.since_edge.as_mut() {
            *since = since.saturating_add(1);
        }
        let phase = match (self.since_edge, self.period) {
            (Some(since), Some(period)) => Some((since as f32 / period as f32).min(CLOCK_HOLD)),
            _ => None,
        };
        (rose, phase)
    }

    /// The clock's frequency over the sample rate, once measured.
    #[inline]
    pub(super) fn frequency(&self) -> Option<f32> {
        self.period.map(|p| 1.0 / p as f32)
    }

    /// Forgets the clock, as when it is unpatched.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: [f32; CHANNELS] = [0.25; CHANNELS];

    #[test]
    fn ad_runs_once_per_trigger_and_rests_at_the_end() {
        let mut ramp = RampGenerator::at_rest();
        let trig = Drive {
            rising: true,
            ..Drive::default()
        };
        ramp.step(
            RampMode::Ad,
            OutputMode::Phase,
            Range::Control,
            0.01,
            &PW,
            trig,
        );
        assert_eq!(ramp.phase(0), 0.0);
        for _ in 0..200 {
            ramp.step(
                RampMode::Ad,
                OutputMode::Phase,
                Range::Control,
                0.01,
                &PW,
                Drive::default(),
            );
        }
        assert_eq!(ramp.phase(0), 1.0);
    }

    #[test]
    fn ar_holds_at_the_top_while_the_gate_is_high() {
        let mut ramp = RampGenerator::at_rest();
        let gate = Drive {
            high: true,
            ..Drive::default()
        };
        for _ in 0..1000 {
            ramp.step(
                RampMode::Ar,
                OutputMode::Phase,
                Range::Control,
                0.01,
                &PW,
                gate,
            );
        }
        assert_eq!(ramp.phase(0), 0.5);
        for _ in 0..1000 {
            ramp.step(
                RampMode::Ar,
                OutputMode::Phase,
                Range::Control,
                0.01,
                &PW,
                Drive::default(),
            );
        }
        assert_eq!(ramp.phase(0), 1.0);
    }

    #[test]
    fn locked_ratios_restart_together_with_the_master() {
        let mut ramp = RampGenerator::default();
        let ratios = [
            Ratio::new(1, 1),
            Ratio::new(1, 2),
            Ratio::new(3, 2),
            Ratio::new(2, 1),
        ];
        ramp.set_next_ratio(ratios);
        let trig = Drive {
            rising: true,
            ..Drive::default()
        };
        ramp.step(
            RampMode::Looping,
            OutputMode::Ratio,
            Range::Control,
            0.01,
            &PW,
            trig,
        );
        // Two master cycles later every output is back at its start.
        for _ in 0..200 {
            ramp.step(
                RampMode::Looping,
                OutputMode::Ratio,
                Range::Control,
                0.01,
                &PW,
                Drive::default(),
            );
        }
        for c in 0..CHANNELS {
            let p = ramp.phase(c);
            assert!(p < 1e-3 || p > 1.0 - 1e-3, "output {c} at {p}");
        }
    }

    #[test]
    fn a_clocked_envelope_at_half_speed_spans_two_periods() {
        let mut ramp = RampGenerator::at_rest();
        ramp.set_next_ratio([
            Ratio::new(1, 1),
            Ratio::new(1, 2),
            Ratio::new(2, 1),
            Ratio::new(1, 4),
        ]);
        let mut clock = ClockFollower::default();
        let period = 100;
        let mut phases = Vec::new();
        for i in 0..1200 {
            let (_, phase) = clock.step(if i % period < 10 { 1.0 } else { 0.0 });
            let drive = Drive {
                clock: phase,
                ..Drive::default()
            };
            ramp.step(
                RampMode::Ad,
                OutputMode::Ratio,
                Range::Control,
                0.01,
                &PW,
                drive,
            );
            phases.push([ramp.phase(1), ramp.phase(2)]);
        }
        // From a whole number of slow cycles in: half speed rises through
        // two periods, double speed restarts mid-period.
        let at = |i: usize| phases[800 + i];
        assert!(at(50)[0] > 0.2 && at(50)[0] < 0.3, "{:?}", at(50));
        assert!(at(150)[0] > 0.7 && at(150)[0] < 0.8, "{:?}", at(150));
        assert!(at(25)[1] > 0.45 && at(25)[1] < 0.55, "{:?}", at(25));
        assert!(at(75)[1] > 0.45 && at(75)[1] < 0.55, "{:?}", at(75));
    }

    #[test]
    fn the_clock_follower_measures_the_period_and_restarts_on_edges() {
        let mut clock = ClockFollower::default();
        let mut phases = Vec::new();
        for i in 0..300 {
            let (_, phase) = clock.step(if i % 100 < 10 { 1.0 } else { 0.0 });
            phases.push(phase);
        }
        assert_eq!(phases[50], None, "no period before the second edge");
        assert_eq!(phases[200], Some(0.0));
        assert_eq!(phases[250], Some(0.5));
        assert_eq!(clock.frequency(), Some(0.01));
    }
}
