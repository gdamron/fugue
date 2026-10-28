//! A request waiting on others applying replaces one waiting for the same
//! control on the same requests, as [`PendingStore`] replaces one due at
//! the same sample: both would apply at one sample, in receipt order, so
//! the higher [`Rank`] stays, and at equal rank the later (last write
//! wins).
//!
//! Only a plain value with no ttl replaces or is replaced (one expiring
//! could leave neither to apply), and only one waiting on
//! requests applying alone: those parts are met at the same sample
//! whenever each started waiting, which a signal's or control's are not.
//!
//! A write kept over a timed edit (see `Publisher::keep_authored`) is
//! copied to apply as the edit lands, if the write itself applied
//! ([`Condition::kept`]). Each copy waits on its own write, so copies
//! replace by control and edit alone: the later copy replaces the one
//! waiting, whatever their ranks, since its write is the author's latest.
//! An author dragging a control a timed edit writes as it lands so holds
//! one request, and one slot, however long the drag. A copy whose write
//! then loses is refused, so neither re-asserts a value that no longer
//! plays. A write waiting on the edit alone for the control (the edit's
//! own, or one scheduled after it) waits beside the copy, to play should
//! the copy break.
//!
//! Writes that could not replace on arrival (that copy and that write,
//! writes on different conditions met at one sample, writes with a ttl)
//! replace as they fall due at one sample, by the same rule, before either
//! reaches the module ([`Conditions::give_way`]): one setter runs for a
//! control at a sample, so a stateful one (a reverb retuning) runs once.
//! A write the pending store holds for that sample weighs in too.
//!
//! [`PendingStore`]: super::super::pending::PendingStore

use super::{Conditions, Entry};
use crate::control_request::condition::{Condition, Predicate};
use crate::control_request::hold::Rank;
use crate::control_request::pending::expired;
use crate::control_request::pending::{Outcome, Outcomes, PendingStore};
use crate::control_request::request::{Request, RequestValue};

/// Whether `request` may replace a waiting request, or be replaced.
fn plain(request: &Request) -> bool {
    !request.event && matches!(request.value, RequestValue::Value(_))
}

fn only_applied(condition: &Condition) -> bool {
    let parts = condition.parts();
    parts
        .iter()
        .all(|part| matches!(part, Predicate::Applied(_)))
}

impl Conditions {
    /// Settles whichever of `request` (timed by `condition`) and a waiting
    /// request it coalesces with loses, the winner taking the waiting
    /// one's place at the end, so in receipt order. Returns `request` when
    /// it coalesces with none. Allocation- and free-free.
    pub(super) fn coalesce(
        &mut self,
        request: Request,
        condition: Condition,
        outcomes: &mut Outcomes,
    ) -> Option<Request> {
        if !plain(&request) || request.expires.is_some() || !only_applied(&condition) {
            return Some(request);
        }
        if let Some(edit) = condition.kept_over() {
            let copy = |entry: &Entry| {
                !entry.broken
                    && plain(&entry.request)
                    && entry.request.expires.is_none()
                    && entry.request.target == request.target
                    && entry.condition.kept_over() == Some(edit)
            };
            // Its parts wait on another write: it leaves, and the new copy
            // waits from the end, so in receipt order.
            if let Some(i) = self.entries.iter().position(copy) {
                let old = self.take(i, &mut outcomes.chains);
                self.settle(old.request, Outcome::Superseded, outcomes);
            }
            return Some(request);
        }
        let same = |entry: &Entry| {
            !entry.broken
                && plain(&entry.request)
                && entry.request.expires.is_none()
                && entry.request.target == request.target
                && entry.condition == condition
        };
        let Some(i) = self.entries.iter().position(same) else {
            return Some(request);
        };
        let new = Rank::of(&request).expect("not an event");
        let waiting = Rank::of(&self.entries[i].request).expect("not an event");
        if new < waiting {
            self.settle(request, new.loses_to(waiting), outcomes);
            return None;
        }
        let mut entry = self.entries.remove(i);
        let old = std::mem::replace(&mut entry.request, request);
        self.entries.push(entry);
        self.settle(old, waiting.loses_to(new), outcomes);
        None
    }
}

impl Conditions {
    /// Settles, before entry `i` (due by `at`) reaches its module, every
    /// other plain write here to the same control due at the same sample,
    /// or entry `i` itself, as two received together replace: the lower
    /// [`Rank`], at equal rank the earlier received (entries are in
    /// receipt order), gives way, so one setter runs for the control at
    /// the sample, the one that would have stood last. Those this table
    /// could not replace on arrival meet here: a kept copy and a write
    /// waiting on its edit alone, writes on different conditions met
    /// together, and writes with a ttl not yet run out. Writes due at
    /// different samples are left to the holds, which weigh each at its
    /// own sample ([`Standing`](crate::control_request::Standing)): at
    /// one sample, a hold that turns away the winner turns away the rest
    /// too, so none is lost to it.
    ///
    /// A write the pending store applies at the same sample (one timed in
    /// samples, applied after this table's at a segment start) weighs the
    /// one left standing the same way, the later received read from the
    /// ids, which a channel hands out in submission order.
    /// Returns where entry `i` now is, or `None` once it gave way.
    /// Allocation- and free-free; one pass over the table per contender.
    pub(super) fn give_way(
        &mut self,
        mut i: usize,
        at: u64,
        pending: &mut PendingStore,
    ) -> Option<usize> {
        let Some(due) = self.contends(i, at) else {
            return Some(i);
        };
        while let Some(other) = self.contender(i, due, at) {
            let mine = Rank::of(&self.entries[i].request).expect("not an event");
            let theirs = Rank::of(&self.entries[other].request).expect("not an event");
            let loser = match mine.cmp(&theirs) {
                std::cmp::Ordering::Less => i,
                std::cmp::Ordering::Greater => other,
                std::cmp::Ordering::Equal => i.min(other),
            };
            let outcome = match loser == i {
                true => mine.loses_to(theirs),
                false => theirs.loses_to(mine),
            };
            self.end(loser, &mut pending.outcomes, outcome);
            if loser == i {
                return None;
            }
            if other < i {
                i -= 1;
            }
        }
        let request = &self.entries[i].request;
        let (target, mine, id) = (request.target, Rank::of(request), request.id);
        let mine = mine.expect("not an event");
        while let Some((place, theirs, their_id)) = pending.rival(&target, due, at) {
            if (mine, id.0) < (theirs, their_id.0) {
                self.end(i, &mut pending.outcomes, mine.loses_to(theirs));
                return None;
            }
            pending.concede(place, theirs.loses_to(mine));
        }
        Some(i)
    }

    /// The sample entry `i` is due at, if it would apply by `at` as a
    /// plain write: not broken, due, and its ttl not run out.
    fn contends(&self, i: usize, at: u64) -> Option<u64> {
        let entry = &self.entries[i];
        let live = !entry.broken && plain(&entry.request) && !expired(&entry.request, at);
        entry.due().filter(|&due| live && due <= at)
    }

    /// Another entry that would apply by `at` to entry `i`'s control, due
    /// at `due` as entry `i` is.
    fn contender(&self, i: usize, due: u64, at: u64) -> Option<usize> {
        let target = self.entries[i].request.target;
        (0..self.entries.len()).find(|&other| {
            other != i
                && self.entries[other].request.target == target
                && self.contends(other, at) == Some(due)
        })
    }
}

#[cfg(test)]
mod pending_tests;
#[cfg(test)]
mod tests;
