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
    /// [`BeatSpec::After`], until the watch is first looked at.
    After(f64),
    /// A position, as of when the clock had begun `beats_before` beats
    /// before its latest reset (see [`Timeline::beats_before`]).
    Count { position: f64, beats_before: u64 },
}

struct Watch {
    request: Request,
    /// Its clock, in the order of its target's generation.
    clock: usize,
    goal: Goal,
}

impl Watch {
    /// The position it waits for, and whether it was set just now (so a
    /// clock already past it has not missed it).
    fn target(&mut self, timeline: &dyn Timeline) -> (f64, bool) {
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
        }
    }
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

    /// Refuses a beat no clock can reach: a span that is not finite, or
    /// negative.
    pub(crate) fn check(request: &Request) -> Result<(), Refusal> {
        match request.when {
            When::Beat(beat) => match beat.spec {
                BeatSpec::After(beats) if beats.is_finite() && beats >= 0.0 => Ok(()),
                BeatSpec::After(_) => Err(Refusal::Invalid),
            },
            _ => Err(Refusal::Unsupported),
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
        };
        self.entries.push(Watch {
            request,
            clock,
            goal,
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
            let watch = self.entries.remove(i);
            outcomes.settle(watch.request, Outcome::Refused(refusal));
        }
    }

    /// One pass at sample `now` over the watches in the `installed`
    /// generation: applies each whose clock's next sample reaches its beat
    /// (or, late, has passed it), and refuses each whose ttl ran out or
    /// whose clock has no timeline. Returns whether any applied, which may
    /// have moved a clock (a reset or tempo timed on beats), so the caller
    /// passes again until none does, and how many samples until the next
    /// may, as far as the clocks' tempi now say. Allocation- and free-free.
    pub(crate) fn pass(
        &mut self,
        now: u64,
        installed: u64,
        host: &mut impl BeatHost,
        outcomes: &mut Outcomes,
    ) -> (bool, Option<u64>) {
        let (mut applied, mut wait) = (false, None::<u64>);
        let mut i = 0;
        while i < self.entries.len() {
            let watch = &mut self.entries[i];
            if watch.request.target.generation != installed {
                i += 1;
                continue;
            }
            if expired(&watch.request, now) {
                let watch = self.entries.remove(i);
                outcomes.settle(watch.request, Outcome::Refused(Refusal::Expired));
                continue;
            }
            let Some(timeline) = host.timeline(watch.clock) else {
                let watch = self.entries.remove(i);
                outcomes.settle(watch.request, Outcome::Refused(Refusal::NoTimeline));
                continue;
            };
            let (beat, fresh) = watch.target(timeline);
            let due = if !fresh && timeline.position() >= beat {
                missed_at(timeline, beat, now)
            } else if timeline.position_after(1) >= beat {
                now
            } else {
                if let Some(samples) = timeline.samples_until(beat) {
                    let samples = samples - 1;
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
