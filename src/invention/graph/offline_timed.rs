//! An offline render's timed requests: the queue and drain a live graph
//! takes its requests through, given to a graph no audio link reaches, so a
//! write timed on a sample or a beat waits, and applies, where a live
//! graph's would, by the same drain (see `super::requests`, `super::beats`).
//!
//! Only the transport differs. With no link to take the queue, the graph
//! takes it itself at each block start ([`SignalGraph::take_offline`]),
//! where a live graph's link takes its own, with the same pop budget, and
//! installs no edit (none is queued to it). What the drain reports back
//! (outcomes, payloads it retired) is received on the control thread, as
//! the render's next control call settles ([`settle_offline`]), never
//! inside a render: in a browser the render is the audio callback.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use super::requests::RequestDrain;
use super::SignalGraph;
use crate::control_request::{
    outcome_channel_reserving, request_channel, Outcome, OutcomeReceiver, Refusal, RequestId,
    RequestSender, When,
};
use crate::invention::publish::timed_write::TIMED_WRITE_ROOM;
use crate::invention::publish::{
    BEAT_WATCH_CAPACITY, OUTCOME_QUEUE_CAPACITY, PAYLOAD_RETIRE_HOLD, PENDING_REQUEST_CAPACITY,
    REQUEST_QUEUE_CAPACITY, TIMED_OUTCOME_RESERVE,
};
use crate::invention::request_log::LoggedBeat;
use crate::payload::{RetireQueue, Retirer};

/// The control side of an offline render's request queue: where its front
/// doors submit timed writes, and what became of each.
pub(crate) struct OfflineRequests {
    /// Where timed writes are submitted.
    pub(crate) sender: RequestSender,
    /// What the drain reports of each request it settles.
    outcomes: OutcomeReceiver,
    /// The payloads the drain retires, freed as it settles.
    payloads: Arc<RetireQueue>,
    /// Timed writes submitted and not yet settled, in submission order.
    waiting: IndexMap<RequestId, OfflineWrite>,
    /// Timed writes settled and not yet taken, oldest first: the latest
    /// [`TIMED_WRITE_ROOM`], as many as may wait at once.
    settled: VecDeque<SettledWrite>,
}

/// A timed write waiting in an offline render.
#[derive(Clone, Debug)]
pub(crate) struct OfflineWrite {
    pub(crate) module_id: String,
    pub(crate) key: String,
    /// When it is timed to apply, as submitted: on a sample (one timed in
    /// samples from now resolved against what had rendered), or a beat.
    pub(crate) when: When,
    /// Its beat, for a write timed in beats, as the request log shows it.
    pub(crate) beat: Option<LoggedBeat>,
}

/// A timed write an offline render settled.
#[derive(Clone, Debug)]
pub(crate) struct SettledWrite {
    pub(crate) id: RequestId,
    pub(crate) write: OfflineWrite,
    pub(crate) outcome: Outcome,
}

/// Gives an offline render's `graph` a request drain sized as a live
/// graph's (see `Publisher::link`), and the control side that submits to
/// it. Its queue holds a live queue's requests but not the room a live one
/// keeps for edits, which an offline render installs in place: so one
/// block's pops empty it, and an edit installed in place maps every
/// request queued. It plays the modules' tables as an untimed render's
/// drain does. Allocates: control thread.
pub(crate) fn offline_timed_requests(graph: &mut SignalGraph) {
    let transport = Arc::clone(&graph.transport);
    let (sender, requests) = request_channel(REQUEST_QUEUE_CAPACITY, transport);
    let payloads = RetireQueue::with_capacity(crate::payload::RETIRE_CAPACITY);
    let (outcome_tx, outcomes) =
        outcome_channel_reserving(OUTCOME_QUEUE_CAPACITY, TIMED_OUTCOME_RESERVE);
    graph.requests = Some(RequestDrain::new(
        requests,
        REQUEST_QUEUE_CAPACITY,
        PENDING_REQUEST_CAPACITY,
        BEAT_WATCH_CAPACITY,
        Retirer::new(Arc::clone(&payloads), PAYLOAD_RETIRE_HOLD),
        outcome_tx,
        Default::default(),
        sender.watch_slots(),
        sender.reserved_queued(),
    ));
    graph.tables_stale = true;
    graph.offline = Some(Box::new(OfflineRequests {
        sender,
        outcomes,
        payloads,
        waiting: IndexMap::new(),
        settled: VecDeque::with_capacity(TIMED_WRITE_ROOM),
    }));
}

impl OfflineRequests {
    /// Notes timed write `id`, just submitted.
    pub(crate) fn submitted(&mut self, id: RequestId, write: OfflineWrite) {
        self.waiting.insert(id, write);
    }

    /// The timed write `id`, while it waits.
    pub(crate) fn waiting_write(&self, id: RequestId) -> Option<&OfflineWrite> {
        self.waiting.get(&id)
    }

    /// How many timed writes wait: those timed on a sample, and all of
    /// them, for `timed_write::check_room`. Settle first.
    pub(crate) fn waiting(&self) -> (usize, usize) {
        let on_sample = (self.waiting.values())
            .filter(|write| matches!(write.when, When::AtSample(_)))
            .count();
        (on_sample, self.waiting.len())
    }

    /// Receives what the drain settled since the last call. The payloads
    /// it retired are freed by [`settle_offline`], off the render's lock.
    /// Control thread, off the render.
    pub(crate) fn settle(&mut self) {
        while let Some((id, outcome, _)) = self.outcomes.try_recv() {
            // Others settled are untimed: a table's, say.
            if let Some(write) = self.waiting.shift_remove(&id) {
                self.keep(SettledWrite { id, write, outcome });
            }
        }
    }

    /// Settles every timed write still waiting as refused: the render they
    /// were queued on is replaced, or failed, so none will apply. Settle
    /// first.
    fn stop(&mut self) {
        for (id, write) in std::mem::take(&mut self.waiting) {
            let outcome = Outcome::Refused(Refusal::Stopped);
            self.keep(SettledWrite { id, write, outcome });
        }
    }

    /// The timed writes settled since the last call, oldest first.
    fn take_settled(&mut self) -> Vec<SettledWrite> {
        self.settled.drain(..).collect()
    }

    fn keep(&mut self, settled: SettledWrite) {
        if self.settled.len() == TIMED_WRITE_ROOM {
            self.settled.pop_front();
        }
        self.settled.push_back(settled);
    }
}

/// Receives what an offline render's `graph` settled since the last call
/// (see [`OfflineRequests::settle`]) and takes each write's settling. With
/// `stop`, or once a render failed (its drain reports nothing more), each
/// write still waiting is settled refused first. The payloads the drain
/// retired are freed once the lock is released, so a render waiting on it
/// never waits on a free. Control thread.
pub(crate) fn settle_offline(graph: &Mutex<SignalGraph>, stop: bool) -> Vec<SettledWrite> {
    let (settled, retired) = {
        let mut graph = graph.lock().unwrap();
        let failed = graph.failed;
        let Some(offline) = graph.offline.as_mut() else {
            return Vec::new();
        };
        offline.settle();
        if stop || failed {
            offline.stop();
        }
        (offline.take_settled(), Arc::clone(&offline.payloads))
    };
    retired.drain();
    settled
}

impl SignalGraph {
    /// The generation an offline render's requests are tagged with: its
    /// own count of the edits it installed in place.
    pub(crate) fn request_generation(&self) -> u64 {
        self.requests.as_ref().map_or(0, |drain| drain.installed)
    }

    /// Takes what an offline render's queue holds, at a block start, as a
    /// live graph's link takes its own (see `publication`): with its pop
    /// budget, and installing no edit. Nothing on a live graph, or an
    /// offline one with no timed requests.
    pub(super) fn take_offline(&mut self) {
        if self.link.is_some() || self.offline.is_none() {
            return;
        }
        let Some(drain) = &self.requests else {
            return;
        };
        let (installed, mut budget) = (drain.installed, drain.pop_limit);
        // No edit is queued to an offline render, and none installs here.
        let edit = self.drain_requests(installed, false, &mut budget, None);
        debug_assert!(edit.is_none(), "an offline render installs no queued edit");
        if let Some(edit) = edit {
            self.keep(edit);
        }
    }
}
