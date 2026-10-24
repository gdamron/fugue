//! What front doors learn of their writes after submitting them: each one
//! that applied, for its `ControlChanged` event, and how far a request got
//! by the time a reply is sent.

use std::collections::VecDeque;

use super::pending::Announcement;
use super::LiveGraph;
use crate::control_request::{Outcome, RequestId};

/// How far a submitted request got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    /// Still waiting to apply.
    Waiting,
    /// Settled, as the outcome says.
    Settled(Outcome),
    /// Applied, with no outcome to say when: one applied as it was made,
    /// a publication installed, or a request whose outcome was lost or
    /// taken, announced as applied as most are (see `PendingLog::settle`).
    Applied,
}

impl LiveGraph {
    /// Each front door's write that applied since the last call, oldest
    /// first.
    pub(crate) fn take_announcements(&self) -> VecDeque<Announcement> {
        self.pending.lock().unwrap().take_announcements()
    }

    /// Whether the audio thread has installed publication `generation`.
    pub(crate) fn installed(&self, generation: u64) -> bool {
        self.publisher.lock().unwrap().applied() >= generation
    }

    /// How far request `id` got.
    pub(crate) fn progress(&self, id: RequestId) -> Progress {
        let mut pending = self.pending.lock().unwrap();
        match pending.outcome_of(id) {
            Some(outcome) => Progress::Settled(outcome),
            None if pending.waiting(id) => Progress::Waiting,
            None => Progress::Applied,
        }
    }
}
