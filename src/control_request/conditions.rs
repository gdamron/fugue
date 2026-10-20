//! Requests timed by a condition ([`When::Condition`]), waiting on the
//! audio thread until it is met.
//!
//! # When a part is met
//!
//! A signal part reads its output's samples as the graph computes them
//! ([`ConditionHost::output`]): every sample once, in order, from the
//! sample the table arms it at. It is met on the exact sample of its
//! `count`th crossing.
//!
//! A control part reads its control where controls change, at segment
//! starts ([`Conditions::start`]): it is met at the first segment start
//! that finds the control across its level from where the last one did.
//!
//! # When a request applies
//!
//! Before its target module computes any sample at or after the one its
//! condition is met at ([`Conditions::apply_due`]): the graph computes the
//! target in pieces split there. So a request applies on the exact sample
//! when every module its condition reads computes before its target. When
//! one does not (it runs after the target, or after it in their feedback
//! group), the condition is known only once the target has computed past
//! that sample: the request applies before the target's next sample and
//! reports [`Outcome::AppliedLate`] with the sample it was due.
//!
//! # Bounded
//!
//! The table never grows past the capacity it is built with. The request
//! channel counts the conditions in flight from submission to settling
//! ([`WatchSlots`]), so a front door can refuse one past
//! [`CONDITION_CAPACITY`] before it is queued; one that arrives at a full
//! table anyway is refused ([`Refusal::PendingFull`]).
//!
//! [`When::Condition`]: super::When::Condition

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use self::parts::{arm, read_controls};
use super::condition::{Condition, Predicate, MAX_PARTS};
use super::hold::Standing;
use super::pending::{expired, Outcome, Outcomes, Refusal};
use super::request::{ControlIndex, ControlTarget, Request, RequestValue, When};
use crate::payload::Retirer;

/// The most requests timed by a condition one graph keeps waiting.
pub(crate) const CONDITION_CAPACITY: usize = 64;

/// Counts the requests timed by a condition that are submitted and not
/// yet settled, shared by a request channel's senders and its drain.
///
/// `Relaxed` is enough: it is a count, and every change is a
/// read-modify-write, so none is lost. A submission counts before it is
/// queued and the drain releases a request only after removing it, so the
/// count never falls below what the drain holds; a reader on a control
/// thread may see a release late, which only refuses conservatively.
#[derive(Debug, Default)]
pub(crate) struct WatchSlots {
    taken: AtomicUsize,
}

impl WatchSlots {
    /// Counts one more request in flight.
    pub(crate) fn take(&self) {
        self.taken.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts one fewer: it settled, or was never queued. Never below zero.
    pub(crate) fn release(&self) {
        let released = self
            .taken
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
        debug_assert!(released.is_ok(), "released a slot never taken");
    }

    /// Whether [`CONDITION_CAPACITY`] requests are in flight.
    pub(crate) fn full(&self) -> bool {
        self.taken.load(Ordering::Relaxed) >= CONDITION_CAPACITY
    }

    pub(crate) fn taken(&self) -> usize {
        self.taken.load(Ordering::Relaxed)
    }
}

/// What conditions need of the graph: the signals they read, and applying
/// a request as a sample-timed one is applied.
pub(crate) trait ConditionHost {
    /// What output `port` of `module` has computed of the current segment:
    /// the sample its first value is at, and the values. `None` when the
    /// module has no such output.
    fn output(&self, module: usize, port: usize) -> Option<(u64, &[f32])>;

    /// The last value output `port` of `module` computed before the
    /// current segment.
    fn last_output(&self, module: usize, port: usize) -> Option<f32>;

    /// The value declared control `control` of `module` holds, as a
    /// number. `None` when it declares no such control.
    fn control(&self, module: usize, control: ControlIndex) -> Option<f32>;

    /// Applies `value` as the pending store's `apply` does, of `standing`
    /// (none for an event): `Err` is what it reports instead.
    fn apply(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        retirer: &mut Retirer,
    ) -> Result<(), Outcome>;
}

/// What one part has seen.
#[derive(Clone, Copy, Debug)]
struct Seen {
    /// The sample it was met at.
    met: Option<u64>,
    /// Crossings still to come.
    remaining: u32,
    /// The last value it read.
    last: f32,
    /// The sample it reads next.
    next: u64,
}

const UNSEEN: Seen = Seen {
    met: None,
    remaining: 0,
    last: 0.0,
    next: 0,
};

struct Entry {
    request: Request,
    /// The request's condition, its modules mapped with its target.
    condition: Condition,
    seen: [Seen; MAX_PARTS],
    /// Whether its parts started reading: at the first segment start in
    /// its target's generation.
    armed: bool,
}

impl Entry {
    /// The sample its condition is met at, once it is.
    fn due(&self) -> Option<u64> {
        let parts = self.condition.parts().len();
        let met = self.seen[..parts].iter().map(|seen| seen.met);
        self.condition.join().met(met)
    }
}

/// Requests timed by a condition, in receipt order. Single-threaded (the
/// audio thread), allocated once on a control thread and never grown.
pub(crate) struct Conditions {
    entries: Vec<Entry>,
    limit: usize,
    slots: Arc<WatchSlots>,
}

impl Conditions {
    /// Room for `capacity` requests, releasing each one's slot in `slots`
    /// as it settles. Allocates: control thread.
    pub(crate) fn new(capacity: usize, slots: Arc<WatchSlots>) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            limit: capacity,
            slots,
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

    /// Refuses a condition nothing can meet, and what v1 does not watch
    /// yet: an edit (FUG-323), or another request.
    pub(crate) fn check(request: &Request) -> Result<(), Refusal> {
        let When::Condition(condition) = &request.when else {
            return Err(Refusal::Unsupported);
        };
        condition.check()?;
        let chained = condition
            .parts()
            .iter()
            .any(|part| matches!(part, Predicate::Applied(_)));
        if request.value.is_edit() || chained {
            return Err(Refusal::Unsupported);
        }
        Ok(())
    }

    /// Settles `request`, a conditional one that leaves the drain here, and
    /// releases its slot.
    pub(crate) fn settle(&self, request: Request, outcome: Outcome, outcomes: &mut Outcomes) {
        outcomes.settle(request, outcome);
        self.slots.release();
    }

    /// Stores `request` ([`Self::check`] has passed it), or settles it
    /// refused ([`Refusal::PendingFull`]) when full. Allocation- and
    /// free-free.
    pub(crate) fn insert(&mut self, request: Request, outcomes: &mut Outcomes) {
        let When::Condition(condition) = request.when else {
            return self.settle(request, Refusal::Unsupported.into(), outcomes);
        };
        if self.is_full() {
            return self.settle(request, Refusal::PendingFull.into(), outcomes);
        }
        self.entries.push(Entry {
            request,
            condition,
            seen: [UNSEEN; MAX_PARTS],
            armed: false,
        });
    }

    /// Removes entry `i`, settling it with `outcome`.
    fn end(&mut self, i: usize, outcomes: &mut Outcomes, outcome: Outcome) {
        let entry = self.entries.remove(i);
        self.settle(entry.request, outcome, outcomes);
    }

    /// At a segment start at `now`: refuses each request in the
    /// `installed` generation whose ttl ran out ([`Refusal::Expired`]),
    /// starts the parts of each new one reading from `now`, and reads the
    /// controls the rest wait on. Allocation- and free-free.
    pub(crate) fn start(
        &mut self,
        now: u64,
        installed: u64,
        host: &impl ConditionHost,
        outcomes: &mut Outcomes,
    ) {
        let mut i = 0;
        while i < self.entries.len() {
            let entry = &mut self.entries[i];
            if entry.request.target.generation != installed {
                i += 1;
                continue;
            }
            if expired(&entry.request, now) {
                self.end(i, outcomes, Refusal::Expired.into());
                continue;
            }
            if !entry.armed && !arm(entry, now, host) {
                self.end(i, outcomes, Refusal::Invalid.into());
                continue;
            }
            entry.armed = true;
            read_controls(entry, now, host);
            i += 1;
        }
    }

    /// Reads what the graph has computed of the current segment into the
    /// parts of each armed request in the `installed` generation (whose
    /// target is `target`, if given). Read them all (`None`) at the end of
    /// every segment, so no sample goes unread. Allocation- and free-free.
    pub(crate) fn read(
        &mut self,
        installed: u64,
        target: Option<usize>,
        host: &impl ConditionHost,
    ) {
        for entry in &mut self.entries {
            let request = &entry.request;
            let ours = target.is_none_or(|target| request.target.module_idx == target);
            if !entry.armed || request.target.generation != installed || !ours {
                continue;
            }
            let parts = entry.condition.parts();
            for (part, seen) in parts.iter().zip(&mut entry.seen) {
                if seen.met.is_none() {
                    read_part(part, seen, host);
                }
            }
        }
    }

    /// Applies, in receipt order, every request in the `installed`
    /// generation (on module `target`, if given) whose condition was met at
    /// or before `at`, the sample its target computes next: on time at its
    /// due sample, late after it. Returns the earliest sample a condition
    /// read so far is met at among the rest, which their target must not
    /// compute past before this is called again. Allocation- and free-free.
    pub(crate) fn apply_due(
        &mut self,
        installed: u64,
        target: Option<usize>,
        at: u64,
        host: &mut impl ConditionHost,
        outcomes: &mut Outcomes,
    ) -> Option<u64> {
        let mut next = None::<u64>;
        let mut i = 0;
        while i < self.entries.len() {
            let entry = &self.entries[i];
            let ours = target.is_none_or(|target| entry.request.target.module_idx == target);
            if entry.request.target.generation != installed || !ours {
                i += 1;
                continue;
            }
            let Some(due) = entry.due() else {
                i += 1;
                continue;
            };
            if due > at {
                next = Some(next.map_or(due, |next| next.min(due)));
                i += 1;
                continue;
            }
            let Entry { request, .. } = self.entries.remove(i);
            if expired(&request, at) {
                self.settle(request, Refusal::Expired.into(), outcomes);
                continue;
            }
            let standing = Standing::of(&request, due);
            let Request {
                target, value, id, ..
            } = request;
            let payload = value.is_payload();
            let outcome = match host.apply(&target, standing, value, &mut outcomes.retirer) {
                Ok(()) if due < at => Outcome::AppliedLate { at, due },
                Ok(()) => Outcome::Applied { at },
                Err(outcome) => outcome,
            };
            outcomes.record(id, payload, outcome);
            self.slots.release();
        }
        next
    }

    /// Maps every request resolved against a generation older than
    /// `installed` into it, its target and the modules its condition reads
    /// alike: `map` gives a module's index in the installed order, or
    /// `None` when it went away, which refuses the request
    /// ([`Refusal::TargetGone`], or [`Refusal::SourceGone`] for a module it
    /// reads). Allocation-free.
    pub(crate) fn remap(
        &mut self,
        installed: u64,
        outcomes: &mut Outcomes,
        mut map: impl FnMut(u64, usize) -> Option<usize>,
    ) {
        let mut i = 0;
        while i < self.entries.len() {
            let entry = &mut self.entries[i];
            let generation = entry.request.target.generation;
            if generation >= installed {
                i += 1;
                continue;
            }
            let Some(module) = map(generation, entry.request.target.module_idx) else {
                self.end(i, outcomes, Refusal::TargetGone.into());
                continue;
            };
            let mut condition = entry.condition;
            let sources = condition.parts_mut().iter_mut();
            let mapped = sources.filter_map(Predicate::module_mut).all(|source| {
                map(generation, *source as usize).is_some_and(|new| {
                    *source = new as u32;
                    true
                })
            });
            if !mapped {
                self.end(i, outcomes, Refusal::SourceGone.into());
                continue;
            }
            entry.condition = condition;
            entry.request.target.generation = installed;
            entry.request.target.module_idx = module;
            i += 1;
        }
    }
}

/// Reads the samples `part` has not yet seen into `seen`.
fn read_part(part: &Predicate, seen: &mut Seen, host: &impl ConditionHost) {
    let Predicate::Signal {
        module,
        port,
        edge,
        level,
        ..
    } = *part
    else {
        return;
    };
    let Some((first, values)) = host.output(module as usize, usize::from(port)) else {
        return;
    };
    debug_assert!(first <= seen.next, "a sample went unread");
    let skip = seen.next.saturating_sub(first) as usize;
    for (offset, &value) in values.iter().enumerate().skip(skip) {
        let crossed = edge.crosses(seen.last, value, level);
        seen.last = value;
        if crossed {
            seen.remaining -= 1;
            if seen.remaining == 0 {
                seen.met = Some(first + offset as u64);
                return;
            }
        }
    }
    seen.next = seen.next.max(first + values.len() as u64);
}

mod parts;
#[cfg(test)]
mod tests;
