//! Control writes submitted to a live graph and not yet settled, for
//! read-back: reads return what was applied, and what is still on its way
//! is listed here.
//!
//! The log is the outcome channel's one consumer: it settles its own
//! writes, keeps every outcome it receives for whoever reports them (see
//! [`PendingLog::take_outcomes`]), and records each applied write in the
//! session's request log (see [`crate::invention::request_log`]).

use std::collections::VecDeque;

use super::publisher::{OUTCOME_QUEUE_CAPACITY, REQUEST_QUEUE_CAPACITY, WAITING_CAPACITY};
use crate::control_request::{EventCursor, Outcome, OutcomeReceiver, RequestId, RtValue};
use crate::invention::request_log::{LoggedBeat, LoggedRequest, Recorder, RequestLog};
use crate::ControlValue;

/// The most writes the log remembers: every request the queue, the
/// pending store and the watches can hold at once.
const LOG_CAPACITY: usize = REQUEST_QUEUE_CAPACITY + WAITING_CAPACITY;

/// A control write waiting to be applied.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingWrite {
    pub(crate) module_id: String,
    pub(crate) key: String,
    pub(crate) value: ControlValue,
    /// The value exactly as the module will receive it, for the log; none
    /// for a payload, which the log cannot carry (it counts as unlogged).
    pub(crate) raw: Option<RtValue>,
    /// Its musical time, when it is timed in beats.
    pub(crate) beat: Option<LoggedBeat>,
    /// Submitted at the automation tier, which takes a control back from a
    /// hold: a replay holds every logged write above automation, so it
    /// cannot reproduce one (it counts as unlogged). Every front door
    /// submits a user's requests until FUG-317 tags sources; one that tags
    /// automation must set this.
    pub(crate) automated: bool,
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
    /// The session's applied writes.
    log: Recorder,
}

impl PendingLog {
    pub(crate) fn new(receiver: OutcomeReceiver) -> Self {
        Self {
            writes: VecDeque::new(),
            outcomes: VecDeque::new(),
            receiver,
            dropped: EventCursor::new(),
            lost: 0,
            log: Recorder::default(),
        }
    }

    /// Receives every outcome waiting, settling the writes they belong to.
    /// When the audio thread had to drop outcomes, which writes they
    /// settled cannot be known, so the log forgets every write it holds
    /// rather than list one as pending for ever.
    pub(crate) fn settle(&mut self) {
        while let Some((id, outcome)) = self.receiver.try_recv() {
            let write = self.writes.iter().position(|(pending, _)| *pending == id);
            let write = write
                .and_then(|at| self.writes.remove(at))
                .map(|(_, write)| write);
            self.record(write, outcome);
            if self.outcomes.len() == OUTCOME_QUEUE_CAPACITY {
                self.outcomes.pop_front();
                self.lost += 1;
            }
            self.outcomes.push_back((id, outcome));
        }
        let dropped = self.dropped.take(self.receiver.dropped());
        if dropped > 0 {
            self.lost += u64::from(dropped);
            self.log.lost(u64::from(dropped));
            self.writes.clear();
        }
    }

    /// Records `write` in the session log when `outcome` applied it, or
    /// counts it lost when the write is no longer remembered.
    fn record(&mut self, write: Option<PendingWrite>, outcome: Outcome) {
        let (sample, due) = match outcome {
            Outcome::Applied { at } => (at, None),
            Outcome::AppliedLate { at, due } => (at, Some(due)),
            Outcome::Superseded | Outcome::Overridden | Outcome::Refused(_) => return,
        };
        let Some(write) = write else {
            self.log.lost(1);
            return;
        };
        let Some(raw) = write.raw.filter(|_| !write.automated) else {
            self.log.unlogged();
            return;
        };
        self.log.applied(LoggedRequest {
            sample,
            module_id: write.module_id,
            key: write.key,
            value: raw.into(),
            due,
            beat: write.beat,
        });
    }

    /// Counts a change committed to the graph, which a replay of the log
    /// does not reproduce.
    pub(crate) fn edited(&mut self) {
        self.log.edited();
    }

    /// Counts a write that reached the graph without a request.
    pub(crate) fn unlogged(&mut self) {
        self.log.unlogged();
    }

    /// The session's request log as recorded so far, every outcome
    /// received first.
    pub(crate) fn request_log(&mut self) -> RequestLog {
        self.settle();
        self.log.export()
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
