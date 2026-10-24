//! The control writes submitted and not yet applied, as a front door lists
//! them.

use super::RunningInvention;
use crate::control_request::{Source, When};
use crate::rpc::{
    PendingCounts, PendingDetail, PendingRequest, PendingRequests, PendingSource, PendingWait,
    COMPACT_PENDING_ITEMS,
};

impl RunningInvention {
    /// The control writes submitted and not yet applied, oldest first,
    /// `source`'s alone when it is given, and how many each source has: in
    /// full, or the oldest [`COMPACT_PENDING_ITEMS`] and a count of the
    /// rest. An authored write kept over a timed edit is also listed as
    /// the copy that lands with the edit, waiting on it.
    pub fn pending_requests(
        &self,
        source: Option<PendingSource>,
        detail: PendingDetail,
    ) -> PendingRequests {
        let mut pending = PendingRequests::default();
        for (id, write) in self.live.pending_requests() {
            let from = pending_source(write.source);
            *count(&mut pending.by_source, from) += 1;
            if source.is_some_and(|source| source != from) {
                continue;
            }
            pending.total += 1;
            if detail == PendingDetail::Compact && pending.items.len() == COMPACT_PENDING_ITEMS {
                continue;
            }
            pending.items.push(PendingRequest {
                request: id.0.to_string(),
                module_id: write.module_id,
                key: write.key,
                requested: write.value,
                source: from,
                waiting: wait(write.when),
            });
        }
        pending.more = pending.total - pending.items.len();
        pending
    }
}

fn pending_source(source: Source) -> PendingSource {
    match source {
        Source::User => PendingSource::User,
        Source::Agent => PendingSource::Agent,
        Source::Script => PendingSource::Script,
        Source::Automation => PendingSource::Automation,
    }
}

fn count(counts: &mut PendingCounts, source: PendingSource) -> &mut usize {
    match source {
        PendingSource::User => &mut counts.user,
        PendingSource::Agent => &mut counts.agent,
        PendingSource::Script => &mut counts.script,
        PendingSource::Automation => &mut counts.automation,
    }
}

fn wait(when: When) -> PendingWait {
    match when {
        When::Now => PendingWait::NextBlock,
        When::AtSample(_) | When::AfterSamples(_) | When::AtTime(_) => PendingWait::Time,
        When::Beat(_) => PendingWait::Beat,
        When::Condition(_) => PendingWait::Condition,
    }
}

#[cfg(test)]
#[test]
fn each_time_a_write_is_due_says_what_it_waits_for() {
    use crate::control_request::{Condition, Predicate, RequestId};
    assert_eq!(wait(When::Now), PendingWait::NextBlock);
    for timed in [When::AtSample(64), When::AfterSamples(64)] {
        assert_eq!(wait(timed), PendingWait::Time);
    }
    let landed = Condition::all(&[Predicate::Applied(RequestId(1))]).unwrap();
    assert_eq!(wait(When::Condition(landed)), PendingWait::Condition);
    assert_eq!(pending_source(Source::Script), PendingSource::Script);
    assert_eq!(
        pending_source(Source::Automation),
        PendingSource::Automation
    );
}
