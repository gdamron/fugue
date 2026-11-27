//! Timed control writes on an offline render, followed as a daemon's are.
//!
//! A write with a `when` waits in the render's request queue and applies
//! where the daemon's drain would apply it (see `graph::offline_timed`).
//! What became of each is received on the next control call, never in a
//! render: an authored one that applied is recorded into the document
//! then, in the order they applied, and before any authored write made
//! since, and each is kept, as the daemon's `ControlRequestSettled` event
//! reports it, for [`RenderEngine::take_settled_writes`]. Loading another
//! invention, or resetting this one, settles every write still waiting as
//! refused: none applies to the piece that replaced the one it was queued
//! on.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::RenderEngine;
use crate::audio_thread::ControlLock;
use crate::invention::graph::{
    settle_offline, AuthoredWrite, LandedEdit, OfflineSettled, SettledWrite, SignalGraph,
};
use crate::invention::orchestration::RuntimeSnapshot;
use crate::invention::publish::timed_write::TIMED_WRITE_ROOM;
use crate::invention::publish::{adopt_offline, reconcile_offline};
use crate::invention::runtime::control_timing::settled_as;
use crate::rpc::ControlRequestSettled;

/// The timed writes settled and not yet taken, oldest first: the latest
/// [`TIMED_WRITE_ROOM`], as many as may wait at once.
#[derive(Default)]
pub(super) struct SettledWrites(ControlLock<VecDeque<ControlRequestSettled>>);

impl RenderEngine {
    /// Every timed write that settled since the last call, as the daemon's
    /// `ControlRequestSettled` event reports it (applied, refused,
    /// overridden...), in the order they settled: the latest few hundred
    /// when none were taken for a while.
    pub fn take_settled_writes(&self) -> Vec<ControlRequestSettled> {
        self.settle_timed_writes();
        self.settled_writes.0.lock().unwrap().drain(..).collect()
    }

    /// Receives what became of the timed writes the last renders settled:
    /// records each authored one that applied, and keeps each for
    /// [`Self::take_settled_writes`]. Control thread, off the render.
    pub(super) fn settle_timed_writes(&self) {
        if let Some(graph) = &self.graph {
            self.keep_settled(graph, settle_offline(graph, true, false));
        }
    }

    /// Settles every timed write still waiting as refused, before the
    /// render's graph is replaced.
    pub(super) fn stop_timed_writes(&self) {
        if let Some(graph) = &self.graph {
            self.keep_settled(graph, settle_offline(graph, true, true));
            self.keep_request_changes();
        }
    }

    fn keep_settled(&self, graph: &Arc<Mutex<SignalGraph>>, settled: OfflineSettled) {
        let landed = settled.landed;
        (self.snapshot()).record_offline_settled(graph, settled.authored, landed);
        if settled.writes.is_empty() {
            return;
        }
        let mut kept = self.settled_writes.0.lock().unwrap();
        for SettledWrite { id, write, outcome } in settled.writes {
            if kept.len() == TIMED_WRITE_ROOM {
                kept.pop_front();
            }
            let written = (write.module_id, write.key);
            kept.push_back(settled_as(id, written, write.beat, Some(outcome)));
        }
    }
}

impl RuntimeSnapshot {
    /// Records the authored timed writes the offline render applied and
    /// the document has yet to record, before an authored write made now:
    /// in the order the render applied them, as the daemon records its own.
    /// Nothing live. Control thread.
    pub(crate) fn settle_offline_authored(&self) {
        let Some(graph) = self.offline.as_ref().and_then(|graph| graph.upgrade()) else {
            return;
        };
        let settled = settle_offline(&graph, false, false);
        self.record_offline_settled(&graph, settled.authored, settled.landed);
    }

    /// Records `authored`, the authored timed writes that applied, and
    /// adopts `landed`, the timed edit that landed among them, in the order
    /// they applied: the writes before it, its change, then those after.
    /// One settled after it but resolved before it landed applied to the
    /// graph it replaced, on its sample: a write to a module it rebuilt or
    /// removed is forgotten, as a live graph's fence forgets it.
    fn record_offline_settled(
        &self,
        graph: &Arc<Mutex<SignalGraph>>,
        mut authored: Vec<AuthoredWrite>,
        landed: Option<LandedEdit>,
    ) {
        let Some(LandedEdit { before, edit }) = landed else {
            return self.record_offline_authored(authored);
        };
        let mut after = authored.split_off(before.min(authored.len()));
        self.record_offline_authored(authored);
        let replaced = edit.replaces;
        let stopped = adopt_offline(graph, edit);
        let replaced = |(module_id, _, _, generation): &AuthoredWrite| {
            *generation <= replaced && stopped.contains(module_id)
        };
        after.retain(|write| !replaced(write));
        self.record_offline_authored(after);
    }

    /// Cancels the timed edit waiting on the offline render, if it
    /// rebuilds or removes `module_id`, whose control `key` an untimed
    /// authored write just set, or writes that control as it lands and the
    /// write is not `spared` (kept over it, or lost), as a live graph's is.
    /// Control thread.
    pub(crate) fn reconcile_offline_edit(&self, control: (&str, &str), spared: bool) {
        if let Some(graph) = self.offline.as_ref().and_then(|graph| graph.upgrade()) {
            reconcile_offline(&graph, control, spared);
        }
    }

    /// Records `authored`, the authored timed writes that applied, in the
    /// order they applied.
    pub(crate) fn record_offline_authored(&self, authored: Vec<AuthoredWrite>) {
        if authored.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        for (module_id, key, value, _) in authored {
            state.record_authored_control(&module_id, &key, &value);
        }
    }
}
