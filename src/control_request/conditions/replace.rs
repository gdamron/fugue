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
//! plays. The edit's own write to the control waits beside the copy, to
//! play should the copy break. As they fall due together the lower rank
//! (at equal rank the write) gives way before either reaches the module
//! ([`Conditions::settle_kept`]), so only one setter runs, as when they
//! coalesced on the copy's arrival.
//!
//! [`PendingStore`]: super::super::pending::PendingStore

use super::{Conditions, Entry};
use crate::control_request::condition::{Condition, Predicate};
use crate::control_request::hold::Rank;
use crate::control_request::pending::{Outcome, Outcomes};
use crate::control_request::request::{Request, RequestId, RequestValue};

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
    /// When entry `i`, due by `at`, is a timed edit's own write (waiting on
    /// the edit alone) or a copy kept over that edit for the same control,
    /// and its partner is due by `at` too (the copy's write applied),
    /// settles whichever loses, as they would have on the copy's arrival:
    /// the lower [`Rank`], at equal rank the write (the copy is the later).
    /// Returns where entry `i` now is, or `None` once it left. Allocation-
    /// and free-free.
    pub(super) fn settle_kept(
        &mut self,
        i: usize,
        at: u64,
        outcomes: &mut Outcomes,
    ) -> Option<usize> {
        let pair = match self.entries[i].condition.kept_over() {
            Some(edit) => self.edit_write(i, edit, at).map(|write| (write, i)),
            None => self.kept_beside(i, at).map(|copy| (i, copy)),
        };
        let Some((write, copy)) = pair else {
            return Some(i);
        };
        let written = Rank::of(&self.entries[write].request).expect("not an event");
        let kept = Rank::of(&self.entries[copy].request).expect("not an event");
        let loser = if written <= kept { write } else { copy };
        let outcome = if loser == write {
            written.loses_to(kept)
        } else {
            kept.loses_to(written)
        };
        self.end(loser, outcomes, outcome);
        match loser.cmp(&i) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Less => Some(i - 1),
            std::cmp::Ordering::Greater => Some(i),
        }
    }

    /// The copy kept over the edit entry `i`'s write waits on alone, for
    /// its control, due by `at`, when entry `i` is such a write.
    fn kept_beside(&self, i: usize, at: u64) -> Option<usize> {
        let entry = &self.entries[i];
        let [Predicate::Applied(edit)] = entry.condition.parts() else {
            return None;
        };
        if !edit_write(entry) {
            return None;
        }
        self.entries.iter().position(|other| {
            !other.broken
                && plain(&other.request)
                && other.request.target == entry.request.target
                && other.condition.kept_over() == Some(*edit)
                && other.due().is_some_and(|due| due <= at)
        })
    }

    /// The write of `edit` itself for the control of copy `i`, due by `at`.
    fn edit_write(&self, i: usize, edit: RequestId, at: u64) -> Option<usize> {
        let copy = &self.entries[i];
        if !plain(&copy.request) {
            return None;
        }
        self.entries.iter().position(|other| {
            !other.broken
                && edit_write(other)
                && other.request.target == copy.request.target
                && other.condition.parts() == [Predicate::Applied(edit)]
                && other.due().is_some_and(|due| due <= at)
        })
    }
}

/// Whether `entry` could be a timed edit's own write: a plain value, not a
/// kept copy, with no ttl of its own.
fn edit_write(entry: &Entry) -> bool {
    entry.condition.kept_over().is_none()
        && plain(&entry.request)
        && entry.request.expires.is_none()
}

#[cfg(test)]
mod tests;
