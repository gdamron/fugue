//! Outcomes on their way back from the audio thread.

use std::sync::Arc;

use super::event::EventCounter;
use super::pending::Outcome;
use super::request::RequestId;
use crate::spsc::{Producer, Ring};

/// A request's outcome, and the sample it latched its clock on when it
/// was timed on a beat and refused or overridden there: the latch held,
/// though nothing applied, so the session's log records it.
pub(crate) type Settled = (RequestId, Outcome, Option<u64>);

/// Creates a channel for up to `capacity` outcomes (rounded up to a power
/// of two). Allocates once: call it on a control thread.
pub(crate) fn outcome_channel(capacity: usize) -> (OutcomeSender, OutcomeReceiver) {
    let queue = Ring::with_capacity(capacity);
    let dropped = Arc::new(EventCounter::new());
    let sender = OutcomeSender {
        producer: Producer::claim(Arc::clone(&queue)),
        dropped: Arc::clone(&dropped),
    };
    (sender, OutcomeReceiver { queue, dropped })
}

/// The audio side's end: the pending store's `Outcomes` sends every
/// settled request's outcome through it.
pub(crate) struct OutcomeSender {
    producer: Producer<Settled>,
    dropped: Arc<EventCounter>,
}

impl OutcomeSender {
    /// Queues an outcome, with the sample a request timed in beats latched
    /// its clock on when it did not apply there (see [`Settled`]), or
    /// counts it dropped when no control thread has made room: the audio
    /// thread never waits for one. Wait-free, allocation-, free- and
    /// lock-free.
    pub(crate) fn send(&mut self, id: RequestId, outcome: Outcome, latched: Option<u64>) {
        if self.producer.push((id, outcome, latched)).is_err() {
            self.dropped.record();
        }
    }
}

/// The control side's end, for front doors that report what became of the
/// requests they submitted. Clones share one queue: each outcome is
/// received once, by whichever clone pops it.
#[derive(Clone)]
pub(crate) struct OutcomeReceiver {
    queue: Arc<Ring<Settled>>,
    dropped: Arc<EventCounter>,
}

impl OutcomeReceiver {
    /// The oldest outcome not yet received. Control thread only: takes the
    /// queue's consumer lock, which the audio side never touches.
    pub(crate) fn try_recv(&self) -> Option<Settled> {
        self.queue.pop()
    }

    /// Outcomes dropped because the queue was full (read it through an
    /// `EventCursor`).
    pub(crate) fn dropped(&self) -> &EventCounter {
        &self.dropped
    }
}
