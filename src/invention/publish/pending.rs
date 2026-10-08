//! Control writes submitted to a live graph and not yet settled, for
//! read-back: reads return what was applied, and what is still on its way
//! is listed here.

use std::collections::VecDeque;

use super::publisher::{PENDING_REQUEST_CAPACITY, REQUEST_QUEUE_CAPACITY};
use crate::control_request::{OutcomeReceiver, RequestId};
use crate::ControlValue;

/// The most writes the log remembers: every request the queue and the
/// pending store can hold. Older entries are forgotten, so a lost outcome
/// never grows the log.
const LOG_CAPACITY: usize = REQUEST_QUEUE_CAPACITY + PENDING_REQUEST_CAPACITY;

/// A control write waiting to be applied.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingWrite {
    pub(crate) module_id: String,
    pub(crate) key: String,
    pub(crate) value: ControlValue,
}

/// Submitted writes, oldest first, until their outcome arrives. Control
/// threads only; the audio thread reports through the outcome channel.
pub(crate) struct PendingLog {
    writes: VecDeque<(RequestId, PendingWrite)>,
    outcomes: OutcomeReceiver,
}

impl PendingLog {
    pub(crate) fn new(outcomes: OutcomeReceiver) -> Self {
        Self {
            writes: VecDeque::new(),
            outcomes,
        }
    }

    /// Records a write submitted as request `id`.
    pub(crate) fn submitted(&mut self, id: RequestId, write: PendingWrite) {
        if self.writes.len() == LOG_CAPACITY {
            self.writes.pop_front();
        }
        self.writes.push_back((id, write));
    }

    /// The writes still waiting, oldest first, once every outcome received
    /// so far has settled its write.
    pub(crate) fn pending(&mut self) -> Vec<PendingWrite> {
        while let Some((id, _)) = self.outcomes.try_recv() {
            self.writes.retain(|(pending, _)| *pending != id);
        }
        self.writes.iter().map(|(_, write)| write.clone()).collect()
    }
}
