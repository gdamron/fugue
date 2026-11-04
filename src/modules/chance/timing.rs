//! When steps fall and how long gates last: the clock's period, the
//! `rate` that divides or subdivides it, and gates that `jitter` delays.

/// Steps per clock pulse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rate {
    /// A step every `every`th pulse.
    every: u32,
    /// Steps per stepping pulse, evenly spaced across the clock's period.
    per: u32,
}

impl Rate {
    pub(super) const ONE: Rate = Rate { every: 1, per: 1 };

    pub(super) fn from_position(position: u32) -> Option<Self> {
        let (every, per) = match position {
            0 => (4, 1),
            1 => (3, 1),
            2 => (2, 1),
            3 => (1, 1),
            4 => (1, 2),
            5 => (1, 3),
            6 => (1, 4),
            _ => return None,
        };
        Some(Self { every, per })
    }
}

/// Follows the clock input and says which samples are steps.
pub(super) struct Clock {
    pub(super) rate: Rate,
    /// A new `rate`, taken on the next pulse so no step is skipped or
    /// doubled mid-pulse.
    pending_rate: Option<Rate>,
    /// Samples since the last pulse rose.
    since_pulse: u32,
    /// Samples between the last two pulses; 0 until two have risen.
    period: u32,
    /// Pulses since the last reset, for division.
    pulses: u32,
    /// The next subdivision of the current pulse to step on.
    next_part: u32,
    seen_pulse: bool,
    /// Subdivisions the last pulse never reached (its period was not yet
    /// known, or the clock sped up), so the bar keeps its place.
    pub(super) missed: u32,
}

/// A step, and whether it fell on a clock pulse's edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    OnPulse,
    Between,
}

impl Clock {
    pub(super) fn new() -> Self {
        Self {
            rate: Rate::ONE,
            pending_rate: None,
            since_pulse: 0,
            period: 0,
            pulses: 0,
            next_part: u32::MAX,
            seen_pulse: false,
            missed: 0,
        }
    }

    /// Changes the rate from the next pulse on; at once before the first.
    pub(super) fn set_rate(&mut self, rate: Rate) {
        match self.seen_pulse {
            true => self.pending_rate = Some(rate),
            false => self.rate = rate,
        }
    }

    /// Starts counting pulses again, so the next one steps.
    pub(super) fn reset(&mut self) {
        self.pulses = 0;
        self.next_part = u32::MAX;
    }

    /// Advances one sample; `rose` is whether a clock pulse rose on it.
    /// Returns the step this sample is, if any.
    pub(super) fn tick(&mut self, rose: bool) -> Option<Step> {
        let mut step = None;
        if rose {
            if self.seen_pulse {
                // A pulse after a long pause (the clock stopped, or slowed
                // past half) leaves the period unknown until the next one,
                // rather than stretching a step over the pause.
                let paused = self.period > 0 && self.since_pulse / 2 > self.period;
                self.period = if paused { 0 } else { self.since_pulse };
            }
            self.seen_pulse = true;
            self.since_pulse = 0;
            // Counted at the rate the last pulse stepped at.
            self.missed = self.rate.per.saturating_sub(self.next_part);
            if let Some(rate) = self.pending_rate.take() {
                self.rate = rate;
            }
            if self.pulses.is_multiple_of(self.rate.every) {
                step = Some(Step::OnPulse);
                self.next_part = 1;
            } else {
                self.next_part = u32::MAX;
            }
            self.pulses = self.pulses.wrapping_add(1);
        } else if self.next_part < self.rate.per && self.period > 0 {
            let due = u64::from(self.period) * u64::from(self.next_part) / u64::from(self.rate.per);
            if u64::from(self.since_pulse) >= due {
                step = Some(Step::Between);
                self.next_part += 1;
            }
        }
        self.since_pulse = self.since_pulse.saturating_add(1);
        step
    }

    /// Whether a pulse holds more than one step, so a gate can't follow
    /// the pulse without running into the next step.
    pub(super) fn subdivides(&self) -> bool {
        self.rate.per > 1
    }

    /// A step's length in samples, once the clock's period is known.
    pub(super) fn step_length(&self) -> Option<u32> {
        let length = u64::from(self.period) * u64::from(self.rate.every) / u64::from(self.rate.per);
        (length > 0).then(|| length.min(u64::from(u32::MAX)) as u32)
    }
}

/// One gate output: high while the clock pulse it fired on is, or for a
/// set number of samples once a delay runs out.
#[derive(Clone, Copy, Default)]
pub(super) struct Gate {
    /// Samples until it rises, when armed.
    delay: Option<u32>,
    /// Samples it stays high for, when not following the clock.
    width: u32,
    remaining: u32,
    follow: bool,
    high: bool,
}

impl Gate {
    /// Fires the gate `delay` samples from now. One that rises with its
    /// clock pulse follows the pulse; any other stays high `width`
    /// samples. A gate still high drops for a sample first, so every
    /// firing is a fresh rising edge.
    pub(super) fn fire(&mut self, delay: u32, width: u32, with_pulse: bool) {
        let delay = if self.high { delay.max(1) } else { delay };
        self.high = false;
        self.delay = Some(delay);
        self.follow = with_pulse && delay == 0;
        self.width = width.max(1);
    }

    /// Advances one sample, returning whether it is high and whether it
    /// rose on this sample.
    pub(super) fn tick(&mut self, clock_high: bool) -> (bool, bool) {
        let mut rose = false;
        match self.delay {
            Some(0) => {
                self.delay = None;
                self.high = true;
                self.remaining = self.width;
                rose = true;
            }
            Some(left) => self.delay = Some(left - 1),
            None => {}
        }
        if self.follow && !clock_high {
            self.high = false;
        }
        let high = self.high;
        if high && !self.follow {
            self.remaining -= 1;
            self.high = self.remaining > 0;
        }
        (high, rose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steps over `pulses` pulses `period` samples apart.
    fn steps(rate: u32, period: usize, pulses: usize) -> Vec<(usize, Step)> {
        let mut clock = Clock::new();
        clock.rate = Rate::from_position(rate).unwrap();
        (0..period * pulses)
            .filter_map(|i| clock.tick(i % period == 0).map(|step| (i, step)))
            .collect()
    }

    #[test]
    fn division_steps_on_every_few_pulses() {
        let at: Vec<usize> = steps(2, 10, 6).iter().map(|s| s.0).collect();
        assert_eq!(at, vec![0, 20, 40]);
    }

    #[test]
    fn subdivision_steps_evenly_once_the_period_is_known() {
        let at = steps(5, 12, 3);
        let samples: Vec<usize> = at.iter().map(|s| s.0).collect();
        assert_eq!(samples, vec![0, 12, 16, 20, 24, 28, 32]);
        assert_eq!(at[1].1, Step::OnPulse);
        assert_eq!(at[2].1, Step::Between);
    }

    #[test]
    fn a_pulse_counts_the_subdivisions_the_last_one_missed() {
        let mut clock = Clock::new();
        clock.rate = Rate::from_position(6).unwrap();
        clock.tick(true);
        assert_eq!(clock.missed, 0);
        (1..8).for_each(|_| assert!(clock.tick(false).is_none()));
        clock.tick(true);
        assert_eq!(clock.missed, 3);
        (1..8).for_each(|_| _ = clock.tick(false));
        clock.tick(true);
        assert_eq!(clock.missed, 0);
    }

    #[test]
    fn a_new_rate_waits_for_the_next_pulse() {
        let mut clock = Clock::new();
        let at: Vec<usize> = (0..400)
            .filter_map(|i| {
                if i == 290 {
                    clock.set_rate(Rate::from_position(6).unwrap());
                }
                clock.tick(i % 100 == 0).map(|_| i)
            })
            .collect();
        assert_eq!(at, vec![0, 100, 200, 300, 325, 350, 375]);
        assert_eq!(clock.missed, 0);
    }

    #[test]
    fn a_pulse_after_a_pause_forgets_the_period() {
        let mut clock = Clock::new();
        clock.rate = Rate::from_position(6).unwrap();
        let mut at = Vec::new();
        for i in 0..1400 {
            let rose = i % 100 == 0 && !(200..1000).contains(&i);
            if clock.tick(rose).is_some() {
                at.push(i);
            }
        }
        // The pause's pulse steps once and keeps no stretched step length;
        // the next pulse measures the tempo again and counts what it missed.
        assert_eq!(&at[..9], &[0, 100, 125, 150, 175, 1000, 1100, 1125, 1150]);
        assert_eq!(clock.step_length(), Some(25));
    }

    #[test]
    fn a_gate_follows_its_pulse_or_lasts_its_width() {
        let mut gate = Gate::default();
        gate.fire(0, 99, true);
        let highs: Vec<bool> = [true, true, false, false]
            .iter()
            .map(|&clock| gate.tick(clock).0)
            .collect();
        assert_eq!(highs, vec![true, true, false, false]);
        gate.fire(2, 3, true);
        let highs: Vec<bool> = (0..7).map(|_| gate.tick(false).0).collect();
        assert_eq!(highs, vec![false, false, true, true, true, false, false]);
    }

    #[test]
    fn a_gate_fired_while_high_drops_first() {
        let mut gate = Gate::default();
        gate.fire(0, 10, false);
        gate.tick(true);
        gate.fire(0, 10, true);
        assert_eq!(gate.tick(true), (false, false));
        assert_eq!(gate.tick(true), (true, true));
    }
}
