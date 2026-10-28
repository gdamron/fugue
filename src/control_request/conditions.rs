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
//! A part waiting on another request is met at the sample that one
//! applies at, as [`Chains`] reports it: when it settles without applying,
//! the request is refused ([`Refusal::ChainBroken`]). One that settled
//! before the part started waiting counts if it is among the latest
//! settles.
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

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use self::parts::{arm, latch, read_controls, waits_on};
use self::replace::Survivor;
use super::chains::Chains;
use super::condition::{Condition, Predicate, MAX_PARTS};
use super::hold::Standing;
use super::pending::{expired, Outcome, Outcomes, PendingStore, Refusal};
use super::request::{ControlIndex, ControlTarget, Request, RequestId, RequestValue, When};
use crate::payload::Retirer;

/// The most requests timed by a condition one graph keeps waiting, but
/// for the [`WAITING_RESERVE`] reserved ones may take too.
pub(crate) const CONDITION_CAPACITY: usize = 64;

/// Room beyond what a condition table or the beat watches hold that only
/// reserved requests (`Request::reserved`: note offs) may take, so a note
/// off timed by a condition or a beat is never shut out by other requests
/// waiting, which may never apply.
pub(crate) const WAITING_RESERVE: usize = 16;

/// Requests timed by a condition one graph keeps waiting, reserved ones
/// included.
pub(crate) const CONDITION_ROOM: usize = CONDITION_CAPACITY + WAITING_RESERVE;

/// A table's `capacity`, and its [`WAITING_RESERVE`] for a `reserved`
/// request.
pub(crate) const fn limit(capacity: usize, reserved: bool) -> usize {
    match reserved {
        true => capacity + WAITING_RESERVE,
        false => capacity,
    }
}

/// How many of the latest timed edits cancelled [`WatchSlots`] remembers.
const CANCELLED: usize = 4;

/// Counts the requests timed by a condition that are submitted and not
/// yet settled, shared by a request channel's senders and its drain.
///
/// `Relaxed` is enough: it is a count, and every change is a
/// read-modify-write, so none is lost. A submission counts before it is
/// queued and the drain releases a request only after removing it, so the
/// count never falls below what the drain holds; a reader on a control
/// thread may see a release late, which only refuses conservatively.
///
/// It also carries the latest timed edits cancelled while they waited, so
/// what waits on one applying is refused ([`Refusal::ChainBroken`]) at the
/// drain's next segment start, and its slot released then, rather than when
/// the edit was due: bars away, or never for an edit timed by a condition.
#[derive(Debug, Default)]
pub(crate) struct WatchSlots {
    taken: AtomicUsize,
    cancelled: [AtomicU64; CANCELLED],
    next_cancelled: AtomicUsize,
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

    /// Whether [`CONDITION_CAPACITY`] requests are in flight, or
    /// [`CONDITION_ROOM`] for a `reserved` one.
    pub(crate) fn full(&self, reserved: bool) -> bool {
        self.taken.load(Ordering::Relaxed) >= limit(CONDITION_CAPACITY, reserved)
    }

    pub(crate) fn taken(&self) -> usize {
        self.taken.load(Ordering::Relaxed)
    }

    /// Control thread: notes that timed edit `id` was cancelled, so never
    /// applies (its fate settled so, once). Only the latest few are kept:
    /// what waits on one forgotten before the drain looked is refused
    /// once the edit is, at its due sample, or at the next segment start
    /// for one timed by a condition (see [`ConditionHost::edit_cancelled`]).
    pub(crate) fn cancel(&self, id: RequestId) {
        let at = self.next_cancelled.fetch_add(1, Ordering::Relaxed) % CANCELLED;
        self.cancelled[at].store(id.0, Ordering::Release);
    }

    /// The latest timed edits cancelled (0: none). Allocation-free.
    fn cancelled(&self) -> [u64; CANCELLED] {
        std::array::from_fn(|at| self.cancelled[at].load(Ordering::Acquire))
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

    /// Whether `value`, a timed edit, was cancelled while it waited, so
    /// never lands. Allocation- and lock-free.
    fn edit_cancelled(&self, _value: &RequestValue) -> bool {
        false
    }

    /// Applies `value` as the pending store's `apply` does, of `standing`
    /// (none for an event): `Err` is what it reports instead.
    fn apply(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        retirer: &mut Retirer,
    ) -> Result<(), Outcome>;

    /// Finishes the late events just applied (see
    /// [`crate::Module::finish_events`]).
    fn finish_events(&mut self) {}
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
    /// A request it waits on settled without applying.
    broken: bool,
}

impl Entry {
    /// The sample its request is due on, once its condition is met.
    fn due(&self) -> Option<u64> {
        let parts = self.condition.parts().len();
        let met = self.seen[..parts].iter().map(|seen| seen.met);
        self.condition.due(met)
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
    /// Room for `capacity` requests, and [`WAITING_RESERVE`] more reserved
    /// ones, releasing each one's slot in `slots` as it settles.
    /// Allocates: control thread.
    pub(crate) fn new(capacity: usize, slots: Arc<WatchSlots>) -> Self {
        Self {
            entries: Vec::with_capacity(capacity + WAITING_RESERVE),
            limit: capacity,
            slots,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The earliest sample after `now` a request of generation `installed`
    /// is due on, its condition met already: one with an offset still to
    /// run (see `Condition::delayed`).
    pub(crate) fn next_due(&self, installed: u64, now: u64) -> Option<u64> {
        (self.entries.iter())
            .filter(|entry| entry.request.target.generation == installed)
            .filter_map(Entry::due)
            .filter(|&due| due > now)
            .min()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether a request, `reserved` or not, finds no room.
    pub(crate) fn is_full(&self, reserved: bool) -> bool {
        self.entries.len() >= limit(self.limit, reserved)
    }

    /// Refuses a condition nothing can meet. An edit timed by one lands
    /// at the segment start it is met at (see `graph::publication::landing`).
    pub(crate) fn check(request: &Request) -> Result<(), Refusal> {
        let When::Condition(condition) = &request.when else {
            return Err(Refusal::Unsupported);
        };
        condition.check()?;
        Ok(())
    }

    /// Settles `request`, a conditional one that leaves the drain here, and
    /// releases its slot.
    pub(crate) fn settle(&self, request: Request, outcome: Outcome, outcomes: &mut Outcomes) {
        outcomes.settle(request, outcome);
        self.slots.release();
    }

    /// Stores `request` ([`Self::check`] has passed it), replacing one it
    /// coalesces with (see [`replace`]), or settles it refused
    /// ([`Refusal::PendingFull`]) when full. Allocation- and free-free.
    pub(crate) fn insert(&mut self, request: Request, outcomes: &mut Outcomes) {
        let When::Condition(condition) = request.when else {
            return self.settle(request, Refusal::Unsupported.into(), outcomes);
        };
        let Some(request) = self.coalesce(request, condition, outcomes) else {
            return;
        };
        if self.is_full(request.reserved) {
            return self.settle(request, Refusal::PendingFull.into(), outcomes);
        }
        self.entries.push(Entry {
            request,
            condition,
            seen: [UNSEEN; MAX_PARTS],
            armed: false,
            broken: false,
        });
    }

    /// Removes entry `i`, its parts no longer waiting on any request.
    fn take(&mut self, i: usize, chains: &mut Chains) -> Entry {
        let entry = self.entries.remove(i);
        if entry.armed {
            for part in entry.condition.parts() {
                if let Predicate::Applied(id) = *part {
                    chains.unwatch(id);
                }
            }
        }
        entry
    }

    /// Removes entry `i`, settling it with `outcome`.
    fn end(&mut self, i: usize, outcomes: &mut Outcomes, outcome: Outcome) {
        let entry = self.take(i, &mut outcomes.chains);
        self.settle(entry.request, outcome, outcomes);
    }

    /// At a segment start at `now`: refuses each request in the
    /// `installed` generation whose ttl ran out ([`Refusal::Expired`]) or
    /// that waits on a request that will not apply
    /// ([`Refusal::ChainBroken`]), starts the parts of each new one reading
    /// from `now`, and reads the controls the rest wait on. Allocation- and
    /// free-free.
    pub(crate) fn start(
        &mut self,
        now: u64,
        installed: u64,
        host: &impl ConditionHost,
        outcomes: &mut Outcomes,
    ) {
        let cancelled = self.slots.cancelled();
        let mut i = 0;
        while i < self.entries.len() {
            let entry = &mut self.entries[i];
            // A timed edit cancelled while its condition waits never lands,
            // nor does what waits on one, whatever graph it is tagged for.
            // The edit is read itself, so it never waits on, however many
            // were cancelled since; what waits on it, through the latest
            // cancelled, or once the edit is refused.
            let value = &entry.request.value;
            if value.is_edit() && host.edit_cancelled(value) {
                self.end(i, outcomes, Refusal::TargetGone.into());
                continue;
            }
            if waits_on(entry, &cancelled) {
                self.end(i, outcomes, Refusal::ChainBroken.into());
                continue;
            }
            if entry.request.target.generation != installed {
                i += 1;
                continue;
            }
            if expired(&entry.request, now) {
                self.end(i, outcomes, Refusal::Expired.into());
                continue;
            }
            if !entry.armed && !arm(entry, now, host, &mut outcomes.chains) {
                self.end(i, outcomes, Refusal::Invalid.into());
                continue;
            }
            entry.armed = true;
            read_controls(entry, now, host);
            latch(entry, &outcomes.chains);
            if entry.broken {
                self.end(i, outcomes, Refusal::ChainBroken.into());
                continue;
            }
            i += 1;
        }
    }

    /// Takes in the requests settled since the parts of each armed request
    /// in the `installed` generation last looked. Allocation- and
    /// free-free.
    pub(crate) fn chain(&mut self, installed: u64, chains: &Chains) {
        for entry in &mut self.entries {
            if entry.armed && entry.request.target.generation == installed {
                latch(entry, chains);
            }
        }
    }

    /// Reads what the graph has computed of the current segment, and the
    /// requests settled, into the parts of each armed request in the
    /// `installed` generation (whose target is `target`, if given). Read
    /// them all (`None`) at the end of every segment, so no sample goes
    /// unread. Allocation- and free-free.
    pub(crate) fn read(
        &mut self,
        installed: u64,
        target: Option<usize>,
        host: &impl ConditionHost,
        chains: &Chains,
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
            latch(entry, chains);
        }
    }

    /// Maps every request resolved against a generation older than
    /// `installed` into it, its target and the modules its condition reads
    /// alike: `map` gives a module's index in the installed order, or
    /// `None` when it went away, which refuses the request
    /// ([`Refusal::TargetGone`], or [`Refusal::SourceGone`] for a module it
    /// reads, unless its condition is already met). Allocation-free.
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
            // A met condition reads nothing more (every part is met), so
            // a source going no longer matters to it: its request stays
            // due, as one waiting for the install that removed it must.
            let met = entry.due().is_some();
            let mut condition = entry.condition;
            let sources = condition.parts_mut().iter_mut();
            let mapped = sources
                .filter_map(Predicate::module_mut)
                .fold(true, |all, source| {
                    let new = map(generation, *source as usize);
                    if let Some(new) = new {
                        *source = new as u32;
                    }
                    all && new.is_some()
                });
            if !mapped && !met {
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

mod apply;
mod parts;
mod replace;
#[cfg(test)]
mod tests;
