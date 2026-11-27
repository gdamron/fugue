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

use super::offline_edits::{LandedEdit, OfflineEdits};
use super::requests::RequestDrain;
use super::SignalGraph;
use crate::control_request::{
    outcome_channel_reserving, request_channel, Outcome, OutcomeReceiver, Refusal, RequestId,
    RequestSender, When,
};
use crate::invention::publish::timed_write::TIMED_WRITE_ROOM;
use crate::invention::publish::CancelCause;
use crate::invention::publish::{
    BEAT_WATCH_CAPACITY, OUTCOME_QUEUE_CAPACITY, PAYLOAD_RETIRE_HOLD, PENDING_REQUEST_CAPACITY,
    REQUEST_QUEUE_CAPACITY, TIMED_OUTCOME_RESERVE,
};
use crate::invention::request_log::LoggedBeat;
use crate::payload::{RetireQueue, Retirer};
use crate::ControlValue;

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
    /// The authored timed writes that applied and the document has yet to
    /// record, in the order they applied, none lost to the cap on
    /// `settled`: no more than may wait at once, as each authored one is
    /// submitted only once those before it were taken (see
    /// `RuntimeSnapshot::record_control`).
    authored: Vec<AuthoredWrite>,
    /// Its timed edits (see `offline_edits`).
    pub(crate) edits: OfflineEdits,
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
    /// The value the document records once it applies, for an authored
    /// write.
    pub(crate) authored: Option<ControlValue>,
    /// The request generation it was resolved against (see
    /// `SignalGraph::request_generation`).
    pub(crate) generation: u64,
    /// Whether its settling is reported (see `take_settled`): a write
    /// chained to a timed edit is not, as a live graph announces none, and
    /// is followed through the edit's status instead.
    pub(crate) announced: bool,
}

/// An authored timed write that applied: its control and value, and the
/// request generation it was resolved against.
pub(crate) type AuthoredWrite = (String, String, ControlValue, u64);

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
        authored: Vec::new(),
        edits: OfflineEdits::default(),
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

    /// Receives what the drain settled since the last call, keeping the
    /// value of each authored write that applied for the document. The
    /// payloads it retired are freed by [`settle_offline`], off the
    /// render's lock. Control thread, off the render.
    pub(crate) fn settle(&mut self) {
        while let Some((id, outcome, _)) = self.outcomes.try_recv() {
            // Others settled are timed edits, or untimed: a table's, say.
            let Some(write) = self.waiting.shift_remove(&id) else {
                self.edits.outcome(id, outcome, self.authored.len());
                continue;
            };
            let applied = matches!(
                outcome,
                Outcome::Applied { .. } | Outcome::AppliedLate { .. }
            );
            if let Some(value) = write.authored.clone().filter(|_| applied) {
                let control = (write.module_id.clone(), write.key.clone());
                (self.authored).push((control.0, control.1, value, write.generation));
            }
            if write.announced {
                self.keep(SettledWrite { id, write, outcome });
            }
        }
    }

    /// Settles every timed write still waiting as refused, and cancels the
    /// timed edit waiting: the render they were queued on is replaced, or
    /// failed, so none will apply. Settle first.
    fn stop(&mut self) {
        self.cancel_edit(|| CancelCause::Stopping);
        for (id, write) in std::mem::take(&mut self.waiting) {
            let outcome = Outcome::Refused(Refusal::Stopped);
            if write.announced {
                self.keep(SettledWrite { id, write, outcome });
            }
        }
    }

    /// Cancels the timed edit waiting for `cause`, unless it has landed or
    /// nothing waits, and tells the drain, as a live graph's publisher
    /// does: writes chained on it settle, and its condition slot is given
    /// back. Returns whether it did.
    pub(crate) fn cancel_edit(&mut self, cause: impl FnOnce() -> CancelCause) -> bool {
        let Some(id) = self.edits.cancel(cause) else {
            return false;
        };
        // The render would refuse the edit by its fate anyway, and its
        // writes with it; this frees their slots as the live publisher
        // does, and is what breaks a chain whose edit was never queued.
        self.sender.cancelled(id);
        true
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

/// What an offline render's timed writes settled, taken from its graph:
/// the authored values to record, in the order they applied, each write's
/// settling when asked for, and the timed edit it landed, to adopt in its
/// place among the authored values.
#[derive(Default)]
pub(crate) struct OfflineSettled {
    pub(crate) authored: Vec<AuthoredWrite>,
    pub(crate) writes: Vec<SettledWrite>,
    pub(crate) landed: Option<LandedEdit>,
}

/// Receives what an offline render's `graph` settled since the last call
/// (see [`OfflineRequests::settle`]) and takes the authored values that
/// applied, the timed edit that landed, and each write's settling too when
/// `writes`. With `stop`, or once a render failed (its drain reports
/// nothing more), each write still waiting is settled refused first, and
/// the timed edit waiting is cancelled, or refused. The payloads the drain
/// retired, and what a timed edit displaced, are freed once the lock is
/// released, so a render waiting on it never waits on a free. Control
/// thread.
pub(crate) fn settle_offline(
    graph: &Mutex<SignalGraph>,
    writes: bool,
    stop: bool,
) -> OfflineSettled {
    let (settled, retired, freed) = {
        let mut graph = graph.lock().unwrap();
        let failed = graph.failed;
        let Some(offline) = graph.offline.as_mut() else {
            return OfflineSettled::default();
        };
        offline.settle();
        if failed {
            offline.edits.strand();
        }
        if stop || failed {
            offline.stop();
        }
        let settled = OfflineSettled {
            authored: std::mem::take(&mut offline.authored),
            writes: if writes {
                offline.take_settled()
            } else {
                Vec::new()
            },
            landed: offline.edits.take_landed(),
        };
        let freed = offline.edits.take_freed();
        (settled, Arc::clone(&offline.payloads), freed)
    };
    retired.drain();
    drop(freed);
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
