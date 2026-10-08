//! The audio thread's store of requests waiting for their sample, and the
//! one point every request leaves the audio thread's hands through.

use super::request::{ControlTarget, Request, RequestValue};
use crate::payload::{Retirer, MAX_RETIRES_PER_REQUEST};

/// Why a request was not applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Its module was removed or rebuilt before it applied.
    TargetGone,
    /// The pending store had no room when it arrived.
    PendingFull,
    /// Its module does not accept it (no module accepts requests until
    /// FUG-310).
    Unsupported,
}

/// How a request left the audio thread's hands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Applied at this sample: its due time, or, when it was already past
    /// on arrival (or when its generation installed), the first segment
    /// start after it.
    Applied {
        at: u64,
    },
    /// Replaced by a later request for the same control and sample.
    Superseded,
    Refused(Refusal),
}

/// Where requests go once settled, and the retirer that takes every
/// payload the audio side does not keep.
///
/// # Retire room
///
/// A payload must never be dropped on the audio thread, and the
/// [`Retirer`] can only promise room it has, so each pending payload
/// request reserves [`MAX_RETIRES_PER_REQUEST`] retirements when it is
/// taken and releases them when it settles. The intake takes a payload
/// request only while [`Self::has_room_for_payload`]: the retirer's room
/// always covers every reservation, so no settle, remap or apply can retire
/// past it. A request carrying a plain value retires nothing and reserves
/// nothing (see [`PendingStore::apply_due`]).
pub(crate) struct Outcomes {
    pub(crate) retirer: Retirer,
    /// Retirements reserved by pending payload requests.
    reserved: usize,
    /// Every outcome settled so far, in order, while there is room: the log
    /// is allocated once and never grows, so tests can count blocks clean.
    #[cfg(test)]
    pub(crate) log: Vec<(super::RequestId, Outcome)>,
}

impl Outcomes {
    fn new(retirer: Retirer) -> Self {
        Self {
            retirer,
            reserved: 0,
            #[cfg(test)]
            log: Vec::with_capacity(4096),
        }
    }

    /// Whether the retirer has room for one more pending payload request
    /// on top of those already reserved.
    pub(crate) fn has_room_for_payload(&self) -> bool {
        self.retirer
            .has_room(self.reserved + MAX_RETIRES_PER_REQUEST)
    }

    /// Reserves retire room for a request just taken: call it before the
    /// request is inserted or settled, and only once
    /// [`Self::has_room_for_payload`] said there is room for a payload.
    pub(crate) fn reserve(&mut self, request: &Request) {
        if request.value.is_payload() {
            self.reserved += MAX_RETIRES_PER_REQUEST;
        }
    }

    /// The single exit point: every request the drain takes leaves through
    /// here exactly once, applied, superseded or refused, and a payload it
    /// still carries is retired. Allocation-, free- and lock-free.
    pub(crate) fn settle(&mut self, request: Request, outcome: Outcome) {
        let Request { value, id, .. } = request;
        let payload = value.is_payload();
        if let RequestValue::Payload(payload) = value {
            self.retirer.retire(payload);
        }
        self.record(id, payload, outcome);
    }

    /// Records the outcome of a request whose value is already disposed of
    /// and releases its reservation. Slice 3 publishes it from here.
    fn record(&mut self, id: super::RequestId, payload: bool, outcome: Outcome) {
        if payload {
            self.reserved -= MAX_RETIRES_PER_REQUEST;
        }
        #[cfg(test)]
        if self.log.len() < self.log.capacity() {
            self.log.push((id, outcome));
        }
        #[cfg(not(test))]
        let _ = (id, outcome);
    }
}

struct Pending {
    request: Request,
    /// The sample it applies at.
    at: u64,
}

/// Requests popped from the queue and not yet applied, in `(at, receipt)`
/// order. Single-threaded (the audio thread), allocated once on a control
/// thread and never grown.
///
/// Receipt order is kept by position, not a stored counter: a new entry is
/// inserted after every entry with the same `at`, so equal times apply in
/// the order they were popped.
///
/// At most one entry per target and `at`: a request for the same control
/// at the same sample replaces the waiting one (last write wins), which is
/// settled [`Outcome::Superseded`]. A remap across a publication may merge
/// two targets afterwards; both then apply in receipt order, which leaves
/// the same value.
///
/// An entry whose target generation is newer than the installed one is
/// held: it is never due, and never bounds a segment, until its
/// publication installs.
pub(crate) struct PendingStore {
    entries: Vec<Pending>,
    limit: usize,
    pub(crate) outcomes: Outcomes,
}

impl PendingStore {
    /// A store for up to `capacity` requests, retiring the payloads it
    /// does not apply through `retirer`. Allocates: control thread.
    pub(crate) fn new(capacity: usize, retirer: Retirer) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            limit: capacity,
            outcomes: Outcomes::new(retirer),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether a request for `target` at `at` would replace a waiting one
    /// (and so needs no room).
    pub(crate) fn coalesces_with(&self, target: &ControlTarget, at: u64) -> bool {
        let first = self.entries.partition_point(|e| e.at < at);
        self.entries[first..]
            .iter()
            .take_while(|e| e.at == at)
            .any(|e| e.request.target == *target)
    }

    /// Whether a request for a new target and time would be refused.
    pub(crate) fn is_full(&self) -> bool {
        self.entries.len() >= self.limit
    }

    /// The capacity reserved at creation; [`Self::insert`] never grows it.
    pub(crate) fn capacity(&self) -> usize {
        self.entries.capacity()
    }

    /// Stores `request` to apply at sample `at`, replacing a waiting request
    /// for the same target and sample, or settles it refused
    /// ([`Refusal::PendingFull`]) when the store is full. Binary search,
    /// then an in-place shift: allocation- and free-free.
    pub(crate) fn insert(&mut self, request: Request, at: u64) {
        let first = self.entries.partition_point(|e| e.at < at);
        let mut end = first + self.entries[first..].partition_point(|e| e.at == at);
        let same = self.entries[first..end]
            .iter()
            .position(|e| e.request.target == request.target);
        if let Some(offset) = same {
            let old = self.entries.remove(first + offset);
            self.outcomes.settle(old.request, Outcome::Superseded);
            end -= 1;
        } else if self.is_full() {
            self.outcomes
                .settle(request, Outcome::Refused(Refusal::PendingFull));
            return;
        }
        self.entries.insert(end, Pending { request, at });
    }

    /// Applies, in order, every entry due at `now` (`at <= now`) whose
    /// target is in the `installed` generation, settling each applied or
    /// refused as `apply` decides. Held entries stay. Compacts in place:
    /// allocation- and free-free.
    ///
    /// `apply` owns the value it is handed. A payload it either keeps or
    /// retires, applied or refused, retiring at most
    /// [`MAX_RETIRES_PER_REQUEST`] values in all (the payload or the value
    /// it replaces); a plain value retires nothing.
    pub(crate) fn apply_due(
        &mut self,
        now: u64,
        installed: u64,
        mut apply: impl FnMut(&ControlTarget, RequestValue, &mut Retirer) -> Result<(), Refusal>,
    ) {
        let due = self.entries.partition_point(|e| e.at <= now);
        let applied = self
            .entries
            .extract_if(..due, |e| e.request.target.generation == installed);
        for entry in applied {
            let Request {
                target, value, id, ..
            } = entry.request;
            let payload = value.is_payload();
            let outcome = match apply(&target, value, &mut self.outcomes.retirer) {
                Ok(()) => Outcome::Applied { at: now },
                Err(refusal) => Outcome::Refused(refusal),
            };
            self.outcomes.record(id, payload, outcome);
        }
    }

    /// The earliest `at` among entries in the `installed` generation.
    /// After [`Self::apply_due`] at `now` it is later than `now`.
    pub(crate) fn next_due(&self, installed: u64) -> Option<u64> {
        self.entries
            .iter()
            .find(|e| e.request.target.generation == installed)
            .map(|e| e.at)
    }

    /// Maps every entry resolved against a generation older than
    /// `installed` into it: `map` gives the module's index in the installed
    /// order, or `None` when its module went away, which settles the entry
    /// refused ([`Refusal::TargetGone`]). Held entries stay as they are.
    /// Allocation- and free-free.
    pub(crate) fn remap(
        &mut self,
        installed: u64,
        mut map: impl FnMut(&ControlTarget) -> Option<usize>,
    ) {
        let gone = self.entries.extract_if(.., |e| {
            let target = &mut e.request.target;
            if target.generation >= installed {
                return false;
            }
            match map(target) {
                Some(module_idx) => {
                    target.generation = installed;
                    target.module_idx = module_idx;
                    false
                }
                None => true,
            }
        });
        for entry in gone {
            self.outcomes
                .settle(entry.request, Outcome::Refused(Refusal::TargetGone));
        }
    }
}

#[cfg(test)]
mod tests;
