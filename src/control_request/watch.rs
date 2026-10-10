//! Requests timed in beats ([`When::Beat`]), waiting on the audio thread
//! for their clock to reach the beat.
//!
//! A beat is resolved as the clock plays, never ahead of it: at each
//! segment start the audio thread asks every watched clock how many
//! samples remain until its beat at the tempo it has now
//! ([`Timeline::samples_until`]) and ends the segment just before that
//! sample, so the next segment starts on it and the request applies there.
//! Every tempo change that lands at a segment start (a request, or
//! automation written before it) is so honoured exactly. One made inside a
//! segment (a scheduler running sample by sample in a feedback group) is
//! seen at the next segment start: the request then applies there, late,
//! and says so ([`Outcome::AppliedLate`]). It never applies early.

use super::pending::{expired, Outcome, Outcomes, Refusal};
use super::request::{BeatSpec, ControlTarget, Request, RequestValue, When};
use super::timeline::Timeline;
use crate::payload::Retirer;

/// What watches need of the graph: its modules' timelines, and applying a
/// request as a sample-timed one is applied.
pub(crate) trait BeatHost {
    fn timeline(&self, module_idx: usize) -> Option<&dyn Timeline>;

    /// Holds the next sample of the clock at `module_idx` at the position
    /// it now predicts: a tempo change applied before that sample takes
    /// effect after it, so the beat a request applies on is still reached
    /// there.
    fn latch(&mut self, module_idx: usize);

    fn apply(
        &mut self,
        target: &ControlTarget,
        value: RequestValue,
        retirer: &mut Retirer,
    ) -> Result<(), Refusal>;
}

/// The beat a watch waits for, in its clock's positions.
#[derive(Clone, Copy)]
enum Goal {
    /// [`BeatSpec::After`], until the watch is first passed over.
    After(f64),
    /// [`Self::Count`] set at the start of the pass that first saw it,
    /// before any request of that pass applied: not yet looked at.
    Armed { position: f64, beats_before: u64 },
    /// A position, as of when the clock had begun `beats_before` beats
    /// before its latest reset (see [`Timeline::beats_before`]).
    Count { position: f64, beats_before: u64 },
    /// [`BeatSpec::Grid`], and the beat it waits for next as of
    /// `beats_before`, once looked for.
    Grid {
        every: f64,
        offset: f64,
        repeat: bool,
        next: Option<(f64, u64)>,
    },
}

struct Watch {
    request: Request,
    /// Its clock, in the order of its target's generation.
    clock: usize,
    goal: Goal,
    /// The sample it last applied at, if it repeats.
    applied: Option<u64>,
}

impl Watch {
    /// The position it waits for, and whether it was set just now (so a
    /// clock already past it has not missed it).
    fn target(&mut self, timeline: &dyn Timeline, now: u64) -> (f64, bool) {
        let begun = timeline.beats_before();
        match self.goal {
            Goal::After(beats) => {
                let position = timeline.position().max(0.0) + beats;
                self.goal = Goal::Count {
                    position,
                    beats_before: begun,
                };
                (position, true)
            }
            // Each reset since jumps the count to the next whole beat.
            Goal::Count {
                position,
                beats_before,
            } => (position - begun.saturating_sub(beats_before) as f64, false),
            Goal::Armed {
                position,
                beats_before,
            } => {
                self.goal = Goal::Count {
                    position,
                    beats_before,
                };
                (position - begun.saturating_sub(beats_before) as f64, true)
            }
            Goal::Grid {
                next: Some((next, beats_before)),
                ..
            } if beats_before == begun => (next, false),
            // Set again after a reset, from the new beat 0.
            Goal::Grid {
                every,
                offset,
                repeat,
                ..
            } => {
                let from = if self.applied == Some(now) {
                    timeline.position_after(1)
                } else {
                    timeline.position()
                };
                let next = grid_after(from, every, offset);
                self.goal = Goal::Grid {
                    every,
                    offset,
                    repeat,
                    next: Some((next, begun)),
                };
                (next, true)
            }
        }
    }

    fn repeats(&self) -> bool {
        matches!(self.goal, Goal::Grid { repeat: true, .. })
    }
}

/// The least `offset + m * every` past `from`.
fn grid_after(from: f64, every: f64, offset: f64) -> f64 {
    let offset = offset.rem_euclid(every);
    let mut m = ((from - offset) / every).floor() + 1.0;
    if offset + (m - 1.0) * every > from {
        m -= 1.0;
    }
    if offset + m * every <= from {
        m += 1.0;
    }
    offset + m * every
}

/// The sample a clock now past `target` reached it at, estimated at its
/// current tempo: before `now`, unless a reset at `now` passed it (the
/// clock has not started since), which is on time.
fn missed_at(timeline: &dyn Timeline, target: f64, now: u64) -> u64 {
    if timeline.position() < 0.0 {
        return now;
    }
    let rate = timeline.position_after(1) - timeline.position();
    let past = if rate > 0.0 && rate.is_finite() {
        ((timeline.position() - target) / rate).floor() as u64
    } else {
        0
    };
    now.saturating_sub(past.saturating_add(1))
}

/// The finest repeating grid, in beats (a 256th note): each application
/// ends a segment, so a finer one would split every block into a few
/// samples for as long as it runs.
pub(crate) const MIN_REPEAT_STEP: f32 = 1.0 / 64.0;

/// Requests waiting for a beat, in receipt order. Single-threaded (the
/// audio thread), allocated once on a control thread and never grown.
pub(crate) struct Watches {
    entries: Vec<Watch>,
    /// The watches it holds at most (the Vec may have more room).
    limit: usize,
}

impl Watches {
    /// Room for `capacity` watches. Allocates: control thread.
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

    pub(crate) fn is_full(&self) -> bool {
        self.entries.len() >= self.limit
    }

    /// Refuses a beat no clock can reach (a span or grid that is not
    /// finite, a negative span, a grid step that is not positive), a
    /// repeating grid finer than [`MIN_REPEAT_STEP`], and a repeating
    /// payload, which applies once.
    pub(crate) fn check(request: &Request) -> Result<(), Refusal> {
        let When::Beat(beat) = request.when else {
            return Err(Refusal::Unsupported);
        };
        match beat.spec {
            BeatSpec::After(beats) if beats.is_finite() && beats >= 0.0 => Ok(()),
            BeatSpec::Grid { every, offset, .. }
                if !(every.is_finite() && every > 0.0 && offset.is_finite()) =>
            {
                Err(Refusal::Invalid)
            }
            BeatSpec::Grid {
                every,
                repeat: true,
                ..
            } if every < MIN_REPEAT_STEP => Err(Refusal::Invalid),
            BeatSpec::Grid { repeat: true, .. } if request.value.is_payload() => {
                Err(Refusal::Unsupported)
            }
            BeatSpec::Grid { .. } => Ok(()),
            BeatSpec::After(_) => Err(Refusal::Invalid),
        }
    }

    /// Stores `request`, timed on `clock` (both in its target's
    /// generation), or settles it refused ([`Refusal::PendingFull`]) when
    /// full. [`Self::check`] has passed it. Allocation- and free-free.
    pub(crate) fn insert(&mut self, request: Request, clock: usize, outcomes: &mut Outcomes) {
        let When::Beat(beat) = request.when else {
            return outcomes.settle(request, Outcome::Refused(Refusal::Unsupported));
        };
        if self.is_full() {
            return outcomes.settle(request, Outcome::Refused(Refusal::PendingFull));
        }
        let goal = match beat.spec {
            BeatSpec::After(beats) => Goal::After(f64::from(beats)),
            BeatSpec::Grid {
                every,
                offset,
                repeat,
            } => Goal::Grid {
                every: f64::from(every),
                offset: f64::from(offset),
                repeat,
                next: None,
            },
        };
        self.entries.push(Watch {
            request,
            clock,
            goal,
            applied: None,
        });
    }

    /// The clocks of the watches in the `installed` generation.
    pub(crate) fn clocks(&self, installed: u64) -> impl Iterator<Item = usize> + '_ {
        self.entries
            .iter()
            .filter(move |watch| watch.request.target.generation == installed)
            .map(|watch| watch.clock)
    }

    /// Maps every watch resolved against a generation older than
    /// `installed` into it, its target and its clock alike: `map` gives a
    /// module's index in the installed order, or `None` when it went away,
    /// which settles the watch refused ([`Refusal::TargetGone`], or
    /// [`Refusal::TimelineGone`] for its clock). Allocation- and free-free.
    pub(crate) fn remap(
        &mut self,
        installed: u64,
        outcomes: &mut Outcomes,
        mut map: impl FnMut(u64, usize) -> Option<usize>,
    ) {
        let mut i = 0;
        while i < self.entries.len() {
            let watch = &mut self.entries[i];
            let generation = watch.request.target.generation;
            if generation >= installed {
                i += 1;
                continue;
            }
            let module = map(generation, watch.request.target.module_idx);
            let clock = map(generation, watch.clock);
            let refusal = match (module, clock) {
                (Some(module), Some(clock)) => {
                    watch.request.target.generation = installed;
                    watch.request.target.module_idx = module;
                    watch.clock = clock;
                    i += 1;
                    continue;
                }
                (None, _) => Refusal::TargetGone,
                (_, None) => Refusal::TimelineGone,
            };
            self.end(i, outcomes, Outcome::Refused(refusal));
        }
    }

    /// Removes watch `i`, settling it with `outcome`, unless it repeats and
    /// has applied: it settled then, at its first application, and ends
    /// silently. Allocation- and free-free.
    fn end(&mut self, i: usize, outcomes: &mut Outcomes, outcome: Outcome) {
        let watch = self.entries.remove(i);
        if watch.applied.is_none() {
            outcomes.settle(watch.request, outcome);
        }
    }

    /// Starts every new span in the `installed` generation counting from
    /// its clock's position now. Called before anything at this sample can
    /// move a clock (a reset, timed in samples or in beats), so a span
    /// counts from where the clock was when the audio thread took it,
    /// whatever applies with it. Allocation- and free-free.
    pub(crate) fn arm(&mut self, installed: u64, host: &impl BeatHost) {
        for watch in &mut self.entries {
            let installed = watch.request.target.generation == installed;
            if let (true, Goal::After(beats)) = (installed, watch.goal) {
                if let Some(timeline) = host.timeline(watch.clock) {
                    watch.goal = Goal::Armed {
                        position: timeline.position().max(0.0) + beats,
                        beats_before: timeline.beats_before(),
                    };
                }
            }
        }
    }

    /// One pass at sample `now` over the watches in the `installed`
    /// generation: applies each whose clock's next sample reaches its beat
    /// (or, late, has passed it), and refuses each whose ttl ran out or
    /// whose clock has no timeline. Returns whether any applied, which may
    /// have moved a clock (a reset or tempo timed on beats), so the caller
    /// passes again until none does, and how many samples until the next
    /// may, as far as the clocks' tempi now say. A repeating watch applies
    /// at most once per sample, and settles at its first application only.
    /// Allocation- and free-free.
    pub(crate) fn pass(
        &mut self,
        now: u64,
        installed: u64,
        host: &mut impl BeatHost,
        outcomes: &mut Outcomes,
    ) -> (bool, Option<u64>) {
        self.arm(installed, host);
        let (mut applied, mut wait) = (false, None::<u64>);
        let mut i = 0;
        while i < self.entries.len() {
            let watch = &mut self.entries[i];
            if watch.request.target.generation != installed {
                i += 1;
                continue;
            }
            if expired(&watch.request, now) {
                self.end(i, outcomes, Outcome::Refused(Refusal::Expired));
                continue;
            }
            let Some(timeline) = host.timeline(watch.clock) else {
                self.end(i, outcomes, Outcome::Refused(Refusal::NoTimeline));
                continue;
            };
            let (beat, fresh) = watch.target(timeline, now);
            if beat.is_nan() || timeline.position().is_nan() {
                // A clock whose position is no number reaches no beat.
                self.end(i, outcomes, Outcome::Refused(Refusal::Invalid));
                continue;
            }
            // A repeat applies once a sample at most, however its grid's
            // arithmetic rounds (a position past f64's integers stands
            // still), so the passes end.
            let again = watch.applied == Some(now);
            let due = if again {
                None
            } else if !fresh && timeline.position() >= beat {
                Some(missed_at(timeline, beat, now))
            } else if timeline.position_after(1) >= beat {
                Some(now)
            } else {
                None
            };
            let Some(due) = due else {
                if let Some(samples) = timeline.samples_until(beat) {
                    // Up to the sample before it reaches the beat; a
                    // repeat that applied now waits a sample at least.
                    let samples = (samples - 1).max(1);
                    wait = Some(wait.map_or(samples, |wait| wait.min(samples)));
                }
                i += 1;
                continue;
            };
            applied = true;
            let outcome = |result: Result<(), Refusal>| match result {
                Ok(()) if due < now => Outcome::AppliedLate { at: now, due },
                Ok(()) => Outcome::Applied { at: now },
                Err(refusal) => Outcome::Refused(refusal),
            };
            if due == now {
                host.latch(watch.clock);
            }
            if let (true, RequestValue::Value(value)) = (watch.repeats(), &watch.request.value) {
                let value = RequestValue::Value(*value);
                let result = host.apply(&watch.request.target, value, &mut outcomes.retirer);
                if result.is_err() {
                    self.end(i, outcomes, outcome(result));
                    continue;
                }
                // Settled at its first application only: the outcome
                // queue holds one per request, and a repeat runs on.
                if watch.applied.is_none() {
                    outcomes.record(watch.request.id, false, outcome(result));
                }
                watch.applied = Some(now);
                if let Goal::Grid { next, .. } = &mut watch.goal {
                    *next = None;
                }
                i += 1;
                continue;
            }
            let Request {
                target, value, id, ..
            } = self.entries.remove(i).request;
            let payload = value.is_payload();
            let result = host.apply(&target, value, &mut outcomes.retirer);
            outcomes.record(id, payload, outcome(result));
        }
        (applied, wait)
    }
}

#[cfg(test)]
mod tests;
