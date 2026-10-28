//! What the condition table asks of the pending store at a segment start,
//! where the two apply as one walk in time order (see
//! `Conditions::apply_due`): to apply what is due before one of its
//! requests, and the writes that contend with one at its sample.

use super::{expired, Pending, PendingStore};
use crate::control_request::hold::Rank;
use crate::control_request::request::{ControlTarget, Request, RequestId, RequestValue};

/// How far [`PendingStore::apply_until`] applies what is due.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Until {
    /// Everything due at or before the sample.
    Through(u64),
    /// Everything due before the sample.
    Before(u64),
    /// Everything due before the sample, and the values due at it, which
    /// an event at that sample reads.
    ValuesAt(u64),
}

impl Until {
    /// Whether an entry due at `at` may apply.
    pub(super) fn reaches(self, at: u64) -> bool {
        match self {
            Self::Through(sample) | Self::ValuesAt(sample) => at <= sample,
            Self::Before(sample) => at < sample,
        }
    }

    /// Whether the events due at `at` may apply, once its values have.
    pub(super) fn events(self, at: u64) -> bool {
        !matches!(self, Self::ValuesAt(sample) if sample == at)
    }
}

impl PendingStore {
    /// The first plain write waiting to `target` at sample `due` (a value,
    /// not an event) whose ttl `now` has not run out: its place, rank and
    /// id. A write the condition table applies at that sample weighs it
    /// (see `Conditions::give_way`). Binary search, then a scan of that
    /// sample's entries.
    pub(crate) fn rival(
        &self,
        target: &ControlTarget,
        due: u64,
        now: u64,
    ) -> Option<(usize, Rank, RequestId)> {
        let first = self.entries.partition_point(|e| e.at < due);
        let rivals = self.entries[first..].iter().take_while(|e| e.at == due);
        rivals
            .enumerate()
            .find_map(|(offset, Pending { request, .. })| {
                let plain = !request.event && matches!(request.value, RequestValue::Value(_));
                let live = request.target == *target && plain && !expired(request, now);
                let rank = Rank::of(request).filter(|_| live)?;
                Some((first + offset, rank, request.id))
            })
    }

    /// Takes out the write at `place` (from [`Self::rival`]), to apply in
    /// the place of the one it outranks there, or to settle. Allocation-
    /// and free-free.
    pub(crate) fn take(&mut self, place: usize) -> Request {
        self.entries.remove(place).request
    }
}
