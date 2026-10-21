//! A request waiting on others applying replaces one waiting for the same
//! control on the same requests, as [`PendingStore`] replaces one due at
//! the same sample: both would apply at one sample, in receipt order, so
//! the higher [`Rank`] stays, and at equal rank the later (last write
//! wins). An author dragging a control a timed edit writes as it lands
//! (see `Publisher::keep_authored`) so holds one request, and one slot,
//! however long the drag.
//!
//! Only a plain value with no ttl replaces, and only one waiting on
//! requests applying alone: those parts are met at the same sample
//! whenever each started waiting, which a signal's or control's are not.
//!
//! [`PendingStore`]: super::super::pending::PendingStore

use super::{Conditions, Entry};
use crate::control_request::condition::{Condition, Predicate};
use crate::control_request::hold::Rank;
use crate::control_request::pending::Outcomes;
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
        let same = |entry: &Entry| {
            !entry.broken
                && plain(&entry.request)
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

#[cfg(test)]
mod tests;
