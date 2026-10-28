//! Applying what the condition table holds, in time order.
//!
//! What is due applies in the order it was due: the earliest due sample
//! first, at each sample its values (a state, a payload) before its events,
//! and then receipt order, as in the pending store. At a segment start the
//! two tables apply as one walk: before a request here applies, the
//! pending store applies what is due before it (and, before an event, the
//! values due beside it). So at one rank the write due last is the one
//! that stays, whichever table holds it, and an event reads what was
//! written for its sample. Writes to one control due at one sample give
//! way to one another first (`Conditions::give_way`).

use super::replace::Survivor;
use super::{ConditionHost, Conditions, Entry};
use crate::control_request::hold::Standing;
use crate::control_request::pending::{expired, Outcome, Outcomes, PendingStore, Refusal, Until};
use crate::control_request::request::{Request, When};

impl Conditions {
    /// Applies, in time order, every request in the `installed` generation
    /// (on module `target`, if given) whose condition was met at or before
    /// `at`, the sample its target computes next: on time at its due
    /// sample, late after it; refuses each that waits on a request that
    /// will not apply. At a segment start (`target` none), what `pending`
    /// holds due before each applies first. Returns the earliest sample a
    /// condition read so far is met at among the rest, which their target
    /// must not compute past before this is called again.
    ///
    /// Each request applied scans the table once to be found, so a block
    /// costs at most about n^2 entry checks for it, n at most
    /// [`super::CONDITION_CAPACITY`], beside those `give_way` makes.
    /// Allocation- and free-free.
    pub(crate) fn apply_due(
        &mut self,
        installed: u64,
        target: Option<usize>,
        at: u64,
        host: &mut impl ConditionHost,
        pending: &mut PendingStore,
    ) -> Option<u64> {
        while let Some((i, due)) = self.earliest(installed, target, at, &mut pending.outcomes) {
            if target.is_none() {
                let until = match self.entries[i].request.fires() {
                    true => Until::ValuesAt(due),
                    false => Until::Before(due),
                };
                catch_up(pending, until, at, installed, host);
            }
            // Writes to one control falling due together, here or in the
            // pending store: all but one give way before any reaches the
            // module.
            let i = match self.give_way(i, at, pending) {
                Some(Survivor::Entry(now)) => now,
                Some(Survivor::Pending(request, due)) => {
                    apply_pending(request, due, at, host, &mut pending.outcomes);
                    continue;
                }
                None => continue,
            };
            let outcomes = &mut pending.outcomes;
            let Entry { request, .. } = self.take(i, &mut outcomes.chains);
            if expired(&request, at) {
                self.settle(request, Refusal::Expired.into(), outcomes);
                continue;
            }
            let standing = Standing::of(&request, due);
            let fires = request.fires();
            let Request {
                target, value, id, ..
            } = request;
            let payload = value.is_payload();
            let outcome = match host.apply(&target, standing, value, &mut outcomes.retirer) {
                Ok(()) if due < at => Outcome::AppliedLate { at, due },
                Ok(()) => Outcome::Applied { at },
                Err(outcome) => outcome,
            };
            // A late event finishes as it applies, as the pending store's.
            if fires && matches!(outcome, Outcome::AppliedLate { .. }) {
                host.finish_events();
            }
            outcomes.record(id, payload, outcome, true);
            self.slots.release();
        }
        (self.entries.iter())
            .filter(|entry| self.ours(entry, installed, target))
            .filter_map(Entry::due)
            .filter(|&due| due > at)
            .min()
    }

    /// The request to apply next among those met by `at` (see
    /// [`Self::apply_due`]), and the sample it is due on. Refuses each
    /// found waiting on a request that will not apply.
    fn earliest(
        &mut self,
        installed: u64,
        target: Option<usize>,
        at: u64,
        outcomes: &mut Outcomes,
    ) -> Option<(usize, u64)> {
        let mut first = None::<(usize, (u64, bool))>;
        let mut i = 0;
        while i < self.entries.len() {
            let entry = &self.entries[i];
            if !self.ours(entry, installed, target) {
                i += 1;
                continue;
            }
            if entry.broken {
                self.end(i, outcomes, Refusal::ChainBroken.into());
                continue;
            }
            if let Some(due) = entry.due().filter(|&due| due <= at) {
                let order = (due, entry.request.fires());
                if first.is_none_or(|(_, earliest)| order < earliest) {
                    first = Some((i, order));
                }
            }
            i += 1;
        }
        first.map(|(i, (due, _))| (i, due))
    }

    /// Whether `entry` is one [`Self::apply_due`] looks at.
    fn ours(&self, entry: &Entry, installed: u64, target: Option<usize>) -> bool {
        let request = &entry.request;
        let on = target.is_none_or(|target| request.target.module_idx == target);
        request.target.generation == installed && on
    }
}

/// Applies what `pending` holds due at `now` as far as `until`, finishing
/// each late event before what follows it.
fn catch_up(
    pending: &mut PendingStore,
    until: Until,
    now: u64,
    installed: u64,
    host: &mut impl ConditionHost,
) {
    loop {
        let finish =
            pending.apply_until(now, installed, until, |target, standing, value, retirer| {
                host.apply(target, standing, value, retirer)
            });
        if !finish {
            return;
        }
        host.finish_events();
    }
}

/// Applies `request`, a plain write the pending store held for sample
/// `due`, at `at` in the place of a write here it outranked, settling it
/// as the pending store would.
fn apply_pending(
    request: Request,
    due: u64,
    at: u64,
    host: &mut impl ConditionHost,
    outcomes: &mut Outcomes,
) {
    let standing = Standing::of(&request, due);
    let timed = request.when != When::Now;
    let Request {
        target, value, id, ..
    } = request;
    let outcome = match host.apply(&target, standing, value, &mut outcomes.retirer) {
        Ok(()) if due < at => Outcome::AppliedLate { at, due },
        Ok(()) => Outcome::Applied { at },
        Err(outcome) => outcome,
    };
    outcomes.record(id, false, outcome, timed);
}

#[cfg(test)]
mod tests;
