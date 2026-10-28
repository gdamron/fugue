//! What became of the requests conditions wait on ([`Predicate::Applied`]):
//! each settle is recorded here as it is reported, so a condition waiting
//! on a request learns the sample it applied at, or that it never will.
//!
//! [`Predicate::Applied`]: super::Predicate::Applied

use super::condition::MAX_PARTS;
use super::conditions::CONDITION_ROOM;
use super::pending::Outcome;
use super::request::RequestId;

/// Settles a store remembers unless told how many it must
/// ([`Chains::new`]).
pub(crate) const RECENT: usize = 256;

/// A request some parts wait on.
#[derive(Clone, Copy, Debug)]
struct Watched {
    id: RequestId,
    /// The parts waiting on it.
    waiters: u32,
    /// What became of it, once it settled.
    outcome: Option<Outcome>,
}

/// The requests parts wait on and the latest settles. Single-threaded (the
/// audio thread), allocated once on a control thread and never grown.
pub(crate) struct Chains {
    /// At most one per part a condition can wait with.
    watched: Vec<Watched>,
    /// The latest settles, `remembered` at most, the oldest overwritten
    /// first.
    recent: Vec<(RequestId, Outcome)>,
    remembered: usize,
    /// Where the next settle goes in `recent` once it is full.
    oldest: usize,
}

impl Chains {
    /// Remembers the latest `remembered` settles, for a condition that
    /// starts waiting on a request after it settled: as many as can settle
    /// between a request settling and a condition submitted after it
    /// starting to wait. Allocates: control thread.
    pub(crate) fn new(remembered: usize) -> Self {
        Self {
            watched: Vec::with_capacity(CONDITION_ROOM * MAX_PARTS),
            recent: Vec::with_capacity(remembered),
            remembered,
            oldest: 0,
        }
    }

    /// Records that request `id` settled with `outcome`. Allocation-free.
    pub(crate) fn settled(&mut self, id: RequestId, outcome: Outcome) {
        if self.recent.len() < self.remembered {
            self.recent.push((id, outcome));
        } else if self.remembered > 0 {
            self.recent[self.oldest] = (id, outcome);
            self.oldest = (self.oldest + 1) % self.remembered;
        }
        if let Some(watched) = self.watched.iter_mut().find(|watched| watched.id == id) {
            watched.outcome = Some(outcome);
        }
    }

    /// Starts one more part waiting on request `id`. Allocation-free:
    /// there is room for every part of every condition.
    pub(crate) fn watch(&mut self, id: RequestId) {
        if let Some(watched) = self.watched.iter_mut().find(|watched| watched.id == id) {
            watched.waiters += 1;
            return;
        }
        debug_assert!(self.watched.len() < self.watched.capacity());
        let recent = self.recent.iter().find(|(settled, _)| *settled == id);
        self.watched.push(Watched {
            id,
            waiters: 1,
            outcome: recent.map(|&(_, outcome)| outcome),
        });
    }

    /// Stops one part waiting on request `id`.
    pub(crate) fn unwatch(&mut self, id: RequestId) {
        let Some(i) = self.watched.iter().position(|watched| watched.id == id) else {
            return;
        };
        self.watched[i].waiters -= 1;
        if self.watched[i].waiters == 0 {
            self.watched.swap_remove(i);
        }
    }

    /// Whether a condition waits on `id` (which then settles at once).
    pub(crate) fn watching(&self, id: RequestId) -> bool {
        self.watched.iter().any(|watched| watched.id == id)
    }

    /// What became of watched request `id`, once it settled.
    pub(crate) fn outcome(&self, id: RequestId) -> Option<Outcome> {
        let watched = self.watched.iter().find(|watched| watched.id == id)?;
        watched.outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_request::Refusal;

    #[test]
    fn a_watched_request_reports_how_it_settled() {
        let mut chains = Chains::new(RECENT);
        chains.watch(RequestId(1));
        chains.watch(RequestId(1));
        assert_eq!(chains.outcome(RequestId(1)), None);
        chains.settled(RequestId(1), Outcome::Applied { at: 7 });
        assert_eq!(
            chains.outcome(RequestId(1)),
            Some(Outcome::Applied { at: 7 })
        );
        chains.unwatch(RequestId(1));
        assert!(chains.outcome(RequestId(1)).is_some());
        chains.unwatch(RequestId(1));
        assert_eq!(chains.outcome(RequestId(1)), None);
    }

    #[test]
    fn a_request_settled_before_it_was_watched_is_remembered_for_a_while() {
        let mut chains = Chains::new(RECENT);
        let refused = Outcome::Refused(Refusal::Expired);
        chains.settled(RequestId(1), refused);
        for id in 2..RECENT as u64 {
            chains.settled(RequestId(id), Outcome::Superseded);
        }
        chains.watch(RequestId(1));
        assert_eq!(chains.outcome(RequestId(1)), Some(refused));
        chains.unwatch(RequestId(1));
        chains.settled(RequestId(RECENT as u64), Outcome::Superseded);
        chains.settled(RequestId(RECENT as u64 + 1), Outcome::Superseded);
        chains.watch(RequestId(1));
        assert_eq!(chains.outcome(RequestId(1)), None);
    }
}
