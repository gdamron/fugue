//! The audio thread's store of requests waiting for their sample, and the
//! one point every request leaves the audio thread's hands through.

use super::chains::{Chains, RECENT};
use super::hold::{Rank, Standing};
use super::outcome::OutcomeSender;
use super::request::{ControlTarget, Request, RequestValue};
use crate::payload::{Retirer, MAX_RETIRES_PER_REQUEST};

/// Why a request was not applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Its module was removed or rebuilt before it applied.
    TargetGone,
    /// The pending store had no room when it arrived.
    PendingFull,
    /// Its module does not accept requests for that control, or not of
    /// that kind (a payload where it declares a value).
    Unsupported,
    /// Its value is not one the control holds (a choice past its options).
    Invalid,
    /// Its `ttl` ran out before it could apply: the sample it would apply
    /// at is past its `expires` sample.
    Expired,
    /// It was timed by wall clock, and the engine has none to place it on
    /// (offline render).
    NoClock,
    /// It was timed in beats on a module that keeps no beat timeline (not
    /// a clock).
    NoTimeline,
    /// It was timed in beats on a clock that was removed or rebuilt before
    /// it applied.
    TimelineGone,
    /// It waited on a condition reading a module that was removed or
    /// rebuilt before it was met.
    SourceGone,
    /// It waited on another request applying, which settled without
    /// applying.
    ChainBroken,
}

/// How a request left the audio thread's hands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Applied exactly at its due sample.
    Applied {
        at: u64,
    },
    /// Applied at `at`, the first segment start after its due sample `due`,
    /// which had already passed when it arrived (or when its generation
    /// installed).
    AppliedLate {
        at: u64,
        due: u64,
    },
    /// Lost to another request of its tier for the same control and
    /// sample: a later one, or one of higher priority.
    Superseded,
    /// Lost to a request of a higher tier for the same control and sample
    /// (see `control_request::hold`).
    Overridden,
    Refused(Refusal),
}

impl From<Refusal> for Outcome {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
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
    sender: OutcomeSender,
    /// Every settle, for the conditions waiting on one.
    pub(crate) chains: Chains,
}

impl Outcomes {
    fn new(retirer: Retirer, sender: OutcomeSender) -> Self {
        Self {
            retirer,
            reserved: 0,
            sender,
            chains: Chains::new(RECENT),
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
        self.settle_on_beat(request, outcome, None);
    }

    /// [`Self::settle`] for a request timed in beats, with the sample it
    /// latched its clock on, if it did and then did not apply.
    pub(crate) fn settle_on_beat(
        &mut self,
        request: Request,
        outcome: Outcome,
        latched: Option<u64>,
    ) {
        let Request { value, id, .. } = request;
        let payload = value.is_payload();
        if let RequestValue::Payload(payload) | RequestValue::Edit(payload) = value {
            self.retirer.retire(payload);
        }
        self.record_on_beat(id, payload, outcome, latched);
    }

    /// Records the outcome of a request whose value is already disposed of,
    /// releases its reservation and sends the outcome to the control side.
    pub(super) fn record(&mut self, id: super::RequestId, payload: bool, outcome: Outcome) {
        self.record_on_beat(id, payload, outcome, None);
    }

    /// [`Self::record`] for a request timed in beats, with the sample it
    /// latched its clock on, if it did and then did not apply.
    pub(super) fn record_on_beat(
        &mut self,
        id: super::RequestId,
        payload: bool,
        outcome: Outcome,
        latched: Option<u64>,
    ) {
        if payload {
            self.reserved -= MAX_RETIRES_PER_REQUEST;
        }
        self.chains.settled(id, outcome);
        self.sender.send(id, outcome, latched);
    }
}

/// Whether `request`, applied at sample `at`, would apply after its
/// `expires` sample.
pub(crate) fn expired(request: &Request, at: u64) -> bool {
    request.expires.is_some_and(|last| at > last)
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
/// the order they were popped, values before events.
///
/// At most one entry per target and `at`: of two requests for the same
/// control at the same sample the higher [`Rank`] stays, and at equal rank
/// the later (last write wins); the other is settled
/// [`Outcome::Superseded`], or [`Outcome::Overridden`] when it lost to a
/// higher tier. A remap across a publication may merge
/// two targets afterwards; both then apply in receipt order, which leaves
/// the same value. Events ([`Request::event`]) never coalesce: two
/// triggers at one sample are two events, and both apply.
///
/// An entry whose target generation is newer than the installed one is
/// held: it is never due, and never bounds a segment, until its
/// publication installs. A held request with a ttl replaces nothing (see
/// [`Self::insert_beside`]): whether it applies is known only at the
/// install, so it must not erase a request that would.
pub(crate) struct PendingStore {
    entries: Vec<Pending>,
    limit: usize,
    pub(crate) outcomes: Outcomes,
}

impl PendingStore {
    /// A store for up to `capacity` requests, retiring the payloads it
    /// does not apply through `retirer` and sending every outcome through
    /// `sender`. Allocates: control thread.
    pub(crate) fn new(capacity: usize, retirer: Retirer, sender: OutcomeSender) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
            limit: capacity,
            outcomes: Outcomes::new(retirer, sender),
        }
    }

    /// Remembers the latest `settles` settles for conditions waiting on
    /// them (see [`Chains::new`]). Allocates: control thread.
    pub(crate) fn remember(&mut self, settles: usize) {
        self.outcomes.chains = Chains::new(settles);
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether a request for `target` at `at` would replace a waiting one
    /// (and so needs no room). An event never does.
    pub(crate) fn coalesces_with(&self, target: &ControlTarget, at: u64, event: bool) -> bool {
        if event {
            return false;
        }
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
    /// for the same target and sample unless it is an event, or settles it refused
    /// ([`Refusal::PendingFull`]) when the store is full; a reserved one
    /// ([`Request::reserved`]) refuses the latest-due request that is not
    /// in its place, and is refused only when every one waiting is
    /// reserved. Binary search, then an in-place shift: allocation- and
    /// free-free.
    pub(crate) fn insert(&mut self, request: Request, at: u64) {
        self.place(request, at, true);
    }

    /// [`Self::insert`] without replacing a waiting request: for a held
    /// request that may expire before its publication installs. Both then
    /// apply in receipt order, which leaves the later value, or the earlier
    /// one when the later expired.
    pub(crate) fn insert_beside(&mut self, request: Request, at: u64) {
        self.place(request, at, false);
    }

    fn place(&mut self, request: Request, at: u64, replace: bool) {
        let first = self.entries.partition_point(|e| e.at < at);
        let mut end = first + self.entries[first..].partition_point(|e| e.at == at);
        let same = self.entries[first..end]
            .iter()
            .position(|e| replace && !request.event && e.request.target == request.target);
        if let Some(offset) = same {
            let waiting = Rank::of(&self.entries[first + offset].request);
            let lost = match (Rank::of(&request), waiting) {
                (Some(new), Some(waiting)) if new < waiting => {
                    return self.outcomes.settle(request, new.loses_to(waiting));
                }
                (Some(new), Some(waiting)) => waiting.loses_to(new),
                _ => Outcome::Superseded,
            };
            let old = self.entries.remove(first + offset);
            self.outcomes.settle(old.request, lost);
            end -= 1;
        } else if self.is_full() {
            // A reserved request takes the place of the latest-due one that
            // is not, refused instead; with none, it is refused itself.
            let displaced = match request.reserved {
                true => self.entries.iter().rposition(|e| !e.request.reserved),
                false => None,
            };
            let Some(displaced) = displaced else {
                self.outcomes
                    .settle(request, Outcome::Refused(Refusal::PendingFull));
                return;
            };
            let old = self.entries.remove(displaced);
            self.outcomes
                .settle(old.request, Outcome::Refused(Refusal::PendingFull));
            if displaced < end {
                end -= 1;
            }
        }
        self.entries.insert(end, Pending { request, at });
    }

    /// Applies, in time order, every entry due at `now` (`at <= now`) whose
    /// target is in the `installed` generation: at each due sample its
    /// values (a state, a payload) in receipt order, then its events (see
    /// [`Request::fires`]), so an event reads what was written for its
    /// sample whichever came first (a note off takes a release written
    /// beside it). Stops after each late event that applies, and says so:
    /// its caller finishes it ([`crate::Module::finish_events`]) and calls
    /// again, so nothing applied after it, by this drain or the next,
    /// reaches back to it, as on every other path.
    /// Hands `apply` its
    /// [`Standing`] (none for an event) and settling each applied, or as
    /// `apply` decides, or refused [`Refusal::Expired`] without
    /// calling `apply` when `now` is past its `expires` sample. Held
    /// entries stay. Compacts in place: allocation- and free-free.
    ///
    /// `apply` owns the value it is handed. A payload it either keeps or
    /// retires, applied or refused, retiring at most
    /// [`MAX_RETIRES_PER_REQUEST`] values in all (the payload or the value
    /// it replaces); a plain value retires nothing.
    pub(crate) fn apply_due(
        &mut self,
        now: u64,
        installed: u64,
        mut apply: impl FnMut(
            &ControlTarget,
            Option<Standing>,
            RequestValue,
            &mut Retirer,
        ) -> Result<(), Outcome>,
    ) -> bool {
        let mut from = 0;
        loop {
            let due = self.entries.partition_point(|e| e.at <= now);
            let Some(first) = (self.entries[from..due].iter())
                .position(|e| e.request.target.generation == installed)
                .map(|offset| from + offset)
            else {
                return false;
            };
            let at = self.entries[first].at;
            let mut fired = false;
            for events in [false, true] {
                let end = first + self.entries[first..].partition_point(|e| e.at == at);
                let applied = self.entries.extract_if(first..end, |e| {
                    e.request.target.generation == installed && e.request.fires() == events
                });
                let one = events && at < now;
                fired |=
                    Self::apply_each(applied, &mut self.outcomes, now, one, &mut apply) && events;
            }
            // What is left at `at` is held.
            from = first + self.entries[first..].partition_point(|e| e.at == at);
            if fired && at < now {
                return true;
            }
        }
    }

    /// Applies each of `applied`, as [`Self::apply_due`] describes, and
    /// says whether any applied; with `one`, stops after the first that
    /// does, leaving the rest in place.
    fn apply_each(
        applied: impl Iterator<Item = Pending>,
        outcomes: &mut Outcomes,
        now: u64,
        one: bool,
        apply: &mut impl FnMut(
            &ControlTarget,
            Option<Standing>,
            RequestValue,
            &mut Retirer,
        ) -> Result<(), Outcome>,
    ) -> bool {
        let mut applied_any = false;
        for Pending { request, at: due } in applied {
            if expired(&request, now) {
                outcomes.settle(request, Outcome::Refused(Refusal::Expired));
                continue;
            }
            let standing = Standing::of(&request, due);
            let Request {
                target, value, id, ..
            } = request;
            let payload = value.is_payload();
            let outcome = match apply(&target, standing, value, &mut outcomes.retirer) {
                Ok(()) if due < now => Outcome::AppliedLate { at: now, due },
                Ok(()) => Outcome::Applied { at: now },
                Err(outcome) => outcome,
            };
            applied_any |= matches!(outcome, Outcome::Applied { .. })
                || matches!(outcome, Outcome::AppliedLate { .. });
            outcomes.record(id, payload, outcome);
            if one && applied_any {
                break;
            }
        }
        applied_any
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
    /// refused ([`Refusal::TargetGone`]). Then settles refused
    /// ([`Refusal::Expired`]) every entry of the installed generation that
    /// can no longer apply by its `expires` sample, due or not: a request
    /// held for this install was never checked, and one far in the future
    /// would otherwise keep its room until its sample. Held entries stay as
    /// they are.
    ///
    /// Runs in each block with an install's remaps in hand: one pass over
    /// the store (at most its capacity), compacting in place. Allocation-
    /// and free-free.
    pub(crate) fn remap(
        &mut self,
        installed: u64,
        now: u64,
        mut map: impl FnMut(&ControlTarget) -> Option<usize>,
    ) {
        let gone = self.entries.extract_if(.., |e| {
            let target = &mut e.request.target;
            if target.generation > installed {
                return false;
            }
            if target.generation < installed {
                let Some(module_idx) = map(target) else {
                    return true;
                };
                target.generation = installed;
                target.module_idx = module_idx;
            }
            expired(&e.request, e.at.max(now))
        });
        for entry in gone {
            // A target left in an older generation went away.
            let refusal = if entry.request.target.generation < installed {
                Refusal::TargetGone
            } else {
                Refusal::Expired
            };
            self.outcomes
                .settle(entry.request, Outcome::Refused(refusal));
        }
    }
}

#[cfg(test)]
mod tests;
