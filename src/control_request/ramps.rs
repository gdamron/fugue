//! Ramps in beats ([`BeatTime::ramp`]): a request carrying one applies
//! where its clock reaches its beat, as any request timed in beats does,
//! and from there glides its control linearly to the request's value,
//! arriving exactly on the sample the clock reaches the ramp's end.
//!
//! # Bounded
//!
//! A ramp runs in the graph's [`Ramps`], a table apart from the watches,
//! preallocated and capped at [`RAMP_CAPACITY`]. A ramp's room is taken as
//! its request is: one that would leave a ramp waiting or running without
//! room is refused ([`Refusal::PendingFull`](super::Refusal::PendingFull))
//! then, never at its beat.
//!
//! # Writes
//!
//! At each segment start the audio thread writes each ramp's value for its
//! clock's position there, and ends the segment just before the nearest
//! ramp's end, so the last write lands on it. Its clock is latched first,
//! so a ramp of its own clock's tempo still reaches its end where it was
//! predicted to.
//!
//! The ramp's writes stand as its request did at its beat, and it runs on
//! only while its control's hold is the one it took there
//! ([`BeatHost::kept`]): any write taking the control since (a higher
//! tier's, or automation's next event on it, a scheduler's entry
//! included) ends it, and the next event replaces it (see
//! `super::hold`). One applying at the same sample at the same rank leaves
//! the hold as it was, so it ends every ramp on a control it reaches
//! directly ([`Ramps::supersede`]).
//!
//! Positions are counted as spans are: a reset of the clock while the
//! ramp runs jumps the count to the next whole beat, so the ramp carries on
//! from there rather than starting over. Values are interpolated in `f64`
//! and kept between the ramp's endpoints, so a ramp between finite values
//! writes only finite values (FUG-302).
//!
//! [`BeatTime::ramp`]: super::BeatTime::ramp

use super::hold::Standing;
use super::request::{ControlTarget, RequestValue, RtValue};
use super::timeline::Timeline;
use super::watch::BeatHost;
use crate::payload::Retirer;

/// The most ramps in beats one graph keeps waiting for their beat or
/// running.
pub(crate) const RAMP_CAPACITY: usize = 64;

/// A ramp's course on its clock's timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BeatRamp {
    from: f32,
    to: f32,
    /// Where it starts and ends, in its clock's positions as of when the
    /// clock had begun `beats_before` beats before its latest reset.
    start: f64,
    end: f64,
    beats_before: u64,
}

impl BeatRamp {
    /// From `from` at position `start` to `to` `beats` later, as of a clock
    /// that has begun `beats_before` beats before its latest reset.
    pub(crate) fn new(from: f32, to: f32, start: f64, beats: f64, beats_before: u64) -> Self {
        Self {
            from,
            to,
            start,
            end: start + beats,
            beats_before,
        }
    }

    /// The beats `timeline` has begun since the ramp's count, over every
    /// reset since it started: each jumps the count to the next whole beat.
    fn resets(&self, timeline: &dyn Timeline) -> f64 {
        timeline.beats_before().saturating_sub(self.beats_before) as f64
    }

    /// What it writes at the clock's next sample, and whether that is its
    /// last write (it has arrived); `None` when the clock's position is no
    /// number.
    pub(crate) fn next(&self, timeline: &dyn Timeline) -> Option<(f32, bool)> {
        let position = timeline.position_after(1) + self.resets(timeline);
        if position.is_nan() {
            return None;
        }
        if position >= self.end {
            return Some((self.to, true));
        }
        let progress = (position - self.start) / (self.end - self.start);
        Some((ramp_value(self.from, self.to, progress), false))
    }

    /// Samples until the clock reaches the ramp's end, at its tempo now.
    pub(crate) fn samples_to_end(&self, timeline: &dyn Timeline) -> Option<u64> {
        timeline.samples_until(self.end - self.resets(timeline))
    }
}

/// `from + (to - from) * progress`, for `progress` clamped to `[0, 1]`:
/// computed in `f64`, where `to - from` cannot overflow, and kept between
/// the endpoints so rounding never overshoots `to`. A NaN `progress`
/// yields `from`.
pub(crate) fn ramp_value(from: f32, to: f32, progress: f64) -> f32 {
    // NaN fails the comparison and `max` takes 0 over it.
    let progress = if progress >= 1.0 {
        1.0
    } else {
        progress.max(0.0)
    };
    let (from_wide, to_wide) = (f64::from(from), f64::from(to));
    let value = (from_wide + (to_wide - from_wide) * progress) as f32;
    value.max(from.min(to)).min(from.max(to))
}

/// A ramp running on its control.
#[derive(Clone, Copy)]
struct Running {
    /// In the installed generation's order, as is `clock`.
    target: ControlTarget,
    clock: usize,
    /// Its request's, at the sample its beat fell on.
    standing: Standing,
    ramp: BeatRamp,
    /// Another write replaced it: removed at the next step.
    ended: bool,
}

/// The ramps running on a graph, in the order they started. Single-
/// threaded (the audio thread), allocated once on a control thread and
/// never grown.
pub(crate) struct Ramps {
    entries: Vec<Running>,
    /// The ramps it holds at most (the Vec may have more room).
    limit: usize,
}

impl Ramps {
    /// Room for `capacity` ramps. Allocates: control thread.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            limit: capacity,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The ramps not yet ended.
    #[cfg(test)]
    pub(crate) fn running(&self) -> usize {
        self.entries.iter().filter(|running| !running.ended).count()
    }

    /// The clocks of the ramps in the `installed` generation.
    pub(crate) fn clocks(&self, installed: u64) -> impl Iterator<Item = usize> + '_ {
        self.entries
            .iter()
            .filter(move |running| running.target.generation == installed)
            .map(|running| running.clock)
    }

    /// Whether one more ramp has room, `waiting` waiting for their beats.
    pub(crate) fn has_room(&self, waiting: usize) -> bool {
        self.entries.len() + waiting < self.limit
    }

    /// Runs `ramp` on `target`'s control, timed on `clock`, writing at
    /// `standing`: it replaces every ramp on a control it reaches. Room
    /// was taken with its request; one without is dropped. Allocation-
    /// and free-free.
    pub(crate) fn start(
        &mut self,
        (target, clock): (ControlTarget, usize),
        standing: Standing,
        ramp: BeatRamp,
        host: &impl BeatHost,
    ) {
        self.supersede(&target, host);
        debug_assert!(self.entries.len() < self.limit, "a ramp's room is taken");
        if self.entries.len() < self.limit {
            self.entries.push(Running {
                target,
                clock,
                standing,
                ramp,
                ended: false,
            });
        }
    }

    /// Ends every ramp on a control `target` reaches: another write
    /// applied to it. Allocation- and free-free.
    pub(crate) fn supersede(&mut self, target: &ControlTarget, host: &impl BeatHost) {
        for running in &mut self.entries {
            if host.overlaps(&running.target, target) {
                running.ended = true;
            }
        }
    }

    /// Maps every ramp into the `installed` generation's order: `map`
    /// gives a module's index there, or `None` when it went away, which
    /// ends the ramp silently (it reported where it started).
    /// Allocation- and free-free.
    pub(crate) fn remap(
        &mut self,
        installed: u64,
        mut map: impl FnMut(u64, usize) -> Option<usize>,
    ) {
        self.entries.retain_mut(|running| {
            let generation = running.target.generation;
            if generation >= installed {
                return true;
            }
            let module = map(generation, running.target.module_idx);
            let clock = map(generation, running.clock);
            let (Some(module), Some(clock)) = (module, clock) else {
                return false;
            };
            running.target.generation = installed;
            running.target.module_idx = module;
            running.clock = clock;
            true
        });
    }

    /// Writes each ramp in the `installed` generation its value at this
    /// sample, ending those that arrive, lose their control or lose their
    /// clock, and removes those ended. Returns how many samples until the
    /// next arrives, as far as the clocks' tempi say after every write:
    /// the segment ends just before it. Allocation- and free-free.
    pub(crate) fn step(
        &mut self,
        installed: u64,
        host: &mut impl BeatHost,
        retirer: &mut Retirer,
    ) -> Option<u64> {
        let mut i = 0;
        while i < self.entries.len() {
            let running = &self.entries[i];
            if running.target.generation != installed && !running.ended {
                i += 1;
                continue;
            }
            let Running {
                target,
                clock,
                standing,
                ramp,
                ended,
            } = *running;
            let next = (host.timeline(clock))
                .and_then(|timeline| ramp.next(timeline))
                .filter(|_| !ended && host.kept(&target, standing));
            let Some((value, last)) = next else {
                self.entries.remove(i);
                continue;
            };
            // Its write may be its clock's tempo: the next sample stays
            // where the clock now predicts, so a last write lands there.
            host.latch(clock);
            let value = RequestValue::Value(RtValue::F32(value));
            let written = host.apply(&target, Some(standing), value, retirer);
            if written.is_err() || last {
                self.entries.remove(i);
                continue;
            }
            i += 1;
        }
        let mut wait = None::<u64>;
        for running in &self.entries {
            if running.target.generation != installed {
                continue;
            }
            let samples = (host.timeline(running.clock))
                .and_then(|timeline| running.ramp.samples_to_end(timeline));
            if let Some(samples) = samples {
                let samples = samples.saturating_sub(1).max(1);
                wait = Some(wait.map_or(samples, |wait| wait.min(samples)));
            }
        }
        wait
    }
}
