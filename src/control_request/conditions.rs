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
//! The table never grows past the capacity it is built with: a request
//! that arrives at a full table is refused ([`Refusal::PendingFull`]).
//!
//! [`When::Condition`]: super::When::Condition

use super::condition::{Condition, Predicate, MAX_PARTS};
use super::hold::Standing;
use super::pending::{expired, Outcome, Outcomes, Refusal};
use super::request::{ControlTarget, Request, RequestValue, When};
use crate::payload::Retirer;

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
}

impl Conditions {
    /// Room for `capacity` requests. Allocates: control thread.
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

    /// Refuses a condition nothing can meet, and what v1 does not watch
    /// yet: an edit (FUG-323), a control's value or another request.
    pub(crate) fn check(request: &Request) -> Result<(), Refusal> {
        let When::Condition(condition) = &request.when else {
            return Err(Refusal::Unsupported);
        };
        condition.check()?;
        let signals = condition
            .parts()
            .iter()
            .all(|part| matches!(part, Predicate::Signal { .. }));
        if request.value.is_edit() || !signals {
            return Err(Refusal::Unsupported);
        }
        Ok(())
    }

    /// Settles `request`, a conditional one that leaves the drain here.
    fn settle(&self, request: Request, outcome: Outcome, outcomes: &mut Outcomes) {
        outcomes.settle(request, outcome);
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
    /// `installed` generation whose ttl ran out ([`Refusal::Expired`]), and
    /// starts the parts of each new one reading from `now`. Allocation- and
    /// free-free.
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
    /// generation for module `target` whose condition was met at or before
    /// `at`, the sample the target computes next: on time at its due
    /// sample, late after it. Returns the earliest sample a condition read
    /// so far is met at among the rest for `target`, which the target must
    /// not compute past before this is called again. Allocation- and
    /// free-free.
    pub(crate) fn apply_due(
        &mut self,
        installed: u64,
        target: usize,
        at: u64,
        host: &mut impl ConditionHost,
        outcomes: &mut Outcomes,
    ) -> Option<u64> {
        let mut next = None::<u64>;
        let mut i = 0;
        while i < self.entries.len() {
            let entry = &self.entries[i];
            let ours = entry.request.target.module_idx == target;
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
        }
        next
    }
}

/// Starts `entry`'s parts reading from `now`: each signal part from its
/// output's last value. `false` when one reads an output its module does
/// not have.
fn arm(entry: &mut Entry, now: u64, host: &impl ConditionHost) -> bool {
    let parts = entry.condition.parts();
    for (part, seen) in parts.iter().zip(&mut entry.seen) {
        if let Predicate::Signal {
            module,
            port,
            count,
            ..
        } = *part
        {
            let Some(last) = host.last_output(module as usize, usize::from(port)) else {
                return false;
            };
            *seen = Seen {
                met: None,
                remaining: count,
                last,
                next: now,
            };
        }
    }
    true
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

#[cfg(test)]
mod tests;
