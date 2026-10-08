//! Control writes submitted to a live graph and not yet settled, for
//! read-back: reads return what was applied, and what is still on its way
//! is listed here.
//!
//! The log is the outcome channel's one consumer: it settles its own
//! writes and keeps every outcome it receives for whoever reports them
//! (see [`PendingLog::take_outcomes`]).

use std::collections::VecDeque;

use super::publisher::{OUTCOME_QUEUE_CAPACITY, PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
use crate::control_request::{EventCursor, Outcome, OutcomeReceiver, RequestId};
use crate::ControlValue;

/// The most writes the log remembers: every request the queue and the
/// pending store can hold at once.
const LOG_CAPACITY: usize = REQUEST_QUEUE_CAPACITY + PENDING_REQUEST_CAPACITY;

/// A control write waiting to be applied.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingWrite {
    pub(crate) module_id: String,
    pub(crate) key: String,
    pub(crate) value: ControlValue,
}

/// Submitted writes, oldest first, until their outcome arrives, and the
/// outcomes received and not yet taken. Control threads only, behind the
/// live graph's lock for it, which a submitter takes under the publisher
/// (publisher, then this).
pub(crate) struct PendingLog {
    writes: VecDeque<(RequestId, PendingWrite)>,
    outcomes: VecDeque<(RequestId, Outcome)>,
    receiver: OutcomeReceiver,
    dropped: EventCursor,
    /// Outcomes lost since they were last taken: dropped by the audio
    /// thread for want of room, or evicted here unreported.
    lost: u64,
}

impl PendingLog {
    pub(crate) fn new(receiver: OutcomeReceiver) -> Self {
        Self {
            writes: VecDeque::new(),
            outcomes: VecDeque::new(),
            receiver,
            dropped: EventCursor::new(),
            lost: 0,
        }
    }

    /// Receives every outcome waiting, settling the writes they belong to.
    /// When the audio thread had to drop outcomes, which writes they
    /// settled cannot be known, so the log forgets every write it holds
    /// rather than list one as pending for ever.
    pub(crate) fn settle(&mut self) {
        while let Some((id, outcome)) = self.receiver.try_recv() {
            self.writes.retain(|(pending, _)| *pending != id);
            if self.outcomes.len() == OUTCOME_QUEUE_CAPACITY {
                self.outcomes.pop_front();
                self.lost += 1;
            }
            self.outcomes.push_back((id, outcome));
        }
        let dropped = self.dropped.take(self.receiver.dropped());
        if dropped > 0 {
            self.lost += u64::from(dropped);
            self.writes.clear();
        }
    }

    /// Records a write just submitted as request `id`. Call it under the
    /// log's lock, taken before the submission, so the outcome cannot be
    /// received before the write is recorded.
    pub(crate) fn submitted(&mut self, id: RequestId, write: PendingWrite) {
        if self.writes.len() == LOG_CAPACITY {
            self.writes.pop_front();
        }
        self.writes.push_back((id, write));
    }

    /// The writes still waiting, oldest first.
    pub(crate) fn pending(&mut self) -> Vec<PendingWrite> {
        self.settle();
        self.writes.iter().map(|(_, write)| write.clone()).collect()
    }

    /// Every outcome received and not yet taken, oldest first, and how
    /// many were lost since the last call (never silently): what the front
    /// doors report.
    pub(crate) fn take_outcomes(&mut self) -> (Vec<(RequestId, Outcome)>, u64) {
        self.settle();
        let lost = std::mem::take(&mut self.lost);
        (self.outcomes.drain(..).collect(), lost)
    }
}
