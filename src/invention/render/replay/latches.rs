//! The clock latches a replay makes apart from its requests': each timed
//! edit's taken on its beat, and each a watch made on its beat before it
//! was refused or overridden there ([`LoggedLatch`](crate::LoggedLatch)).

use crate::{LoggedEditOutcome, LoggedWhen, RequestLog};

/// A clock to latch once the render reaches `sample` and this many logged
/// requests have applied, as live latched it.
pub(super) struct DueLatch {
    pub(super) sample: u64,
    pub(super) requests_before: usize,
    pub(super) clock: String,
}

/// Every latch in `log` apart from its requests', in the order live made
/// them.
pub(super) fn build(log: &RequestLog) -> Vec<DueLatch> {
    // Live latches a clock for an edit taken on its beat, not for one
    // taken late.
    let edits = log
        .timed_edits
        .iter()
        .filter_map(|edit| match (&edit.when, &edit.outcome) {
            (LoggedWhen::Beat { beat }, LoggedEditOutcome::Landed { sample, due: None }) => {
                Some(DueLatch {
                    sample: *sample,
                    requests_before: edit.requests_before,
                    clock: beat.clock().to_string(),
                })
            }
            _ => None,
        });
    let refused = log.latches.iter().map(|latch| DueLatch {
        sample: latch.sample,
        requests_before: latch.requests_before,
        clock: latch.clock.clone(),
    });
    let mut latches: Vec<DueLatch> = edits.chain(refused).collect();
    // Both lists are in live order; a clock latches once a sample, so two
    // latches between the same requests may go either way.
    latches.sort_by_key(|latch| (latch.sample, latch.requests_before));
    latches
}
