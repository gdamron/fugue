//! Timed edit batches on an offline render, followed as a daemon's are.
//!
//! A batch with a `when` is checked and prepared now, as an untimed one
//! is, and waits in the render's request queue to land where the daemon's
//! would (see `publish::offline_timed`): on its sample, or on its beat as
//! the render's clock plays it. Its status reads as the daemon's
//! `GetRequest` reads, and changes as later renders and control calls
//! settle it: queued, landed in a render, adopted into the document on
//! the next control call; or cancelled, refused or expired. Loading
//! another invention, or resetting this one, cancels the one waiting, and
//! request ids start again with the next invention, as each of a daemon's
//! inventions numbers its own.

use std::collections::{HashSet, VecDeque};

use super::batch::{apply_all, batch_refusal, builds_module, check_ops};
use super::RenderEngine;
use crate::audio_thread::ControlLock;
use crate::control_request::RequestId;
use crate::invention::audio_assets;
use crate::invention::edits::EditedCandidate;
use crate::invention::graph::SignalGraph;
use crate::invention::publish::{queue_offline, QueueRefused, LEDGER_CAPACITY};
use crate::invention::runtime::control_timing::ttl_samples;
use crate::invention::runtime::timed::{edit_time, parse_id, wire_status};
use crate::rpc::RpcError;
use crate::rpc::{check_edit_batch, EditWhen, RequestStatus, StructuralEdit};
use crate::ControlValue;

/// The statuses of a replaced invention's timed edits that changed before
/// it was replaced, not yet taken: its latest few dozen.
#[derive(Default)]
pub(super) struct EditChanges(ControlLock<VecDeque<RequestStatus>>);

impl RenderEngine {
    /// [`Self::apply_edits`], landing `when` rather than at once, unless
    /// `ttl_ms` milliseconds pass first, as a daemon's timed `ApplyEdits`
    /// does (see
    /// [`RunningInvention::apply_edits_at`](crate::RunningInvention::apply_edits_at)).
    /// Returns the queued request's status. A beat time counts on the
    /// clock it names, otherwise on the invention's first clock.
    ///
    /// Refused as an untimed batch is, and also, with nothing queued, when
    /// it changes no structure, adds or removes a code or agent module,
    /// names a time that can never come, writes a control that cannot be
    /// timed (an undeclared control), or another timed edit waits.
    ///
    /// Its writes to modules it keeps play from the sample it lands on, and
    /// are recorded in the document as they apply; those to modules it adds
    /// or rebuilds are made as they are built.
    ///
    /// While it waits, any other edit or batch cancels it, as does an
    /// authored write to a module it rebuilds or removes, or loading or
    /// resetting the render; pausing keeps it. An authored write to a
    /// control it writes as it lands is kept over it: the author's value
    /// plays on. Follow it with
    /// [`Self::request_status`] or [`Self::take_request_changes`].
    pub fn apply_edits_at(
        &self,
        edits: &[StructuralEdit],
        when: &EditWhen,
        ttl_ms: Option<u64>,
    ) -> Result<RequestStatus, RpcError> {
        check_edit_batch(edits)?;
        let time = edit_time(when, None)?;
        // What landed and applied is recorded before the batch is planned.
        self.settle_timed_writes();
        check_ops(edits)?;
        let graph = self.loaded().map_err(RpcError::from)?;
        let candidate = self.checked_writes(edits)?;
        let mut built = self.build_added(edits, candidate.as_ref())?;
        let writes = (candidate.as_ref()).map_or_else(Vec::new, |c| kept_writes(edits, c));
        let ttl = ttl_samples(self.sample_rate, ttl_ms);
        let mut failure = None;
        let mut swapped = Vec::new();
        let targets = (
            &self.control_surfaces,
            &self.module_ports,
            &self.state,
            &self.handles,
        );
        let queued = queue_offline(graph, targets, time, ttl, &writes, |change| {
            apply_all(change, edits, &mut built, &mut swapped, &mut failure)
        });
        // A module built for an edit that never got its turn drops here,
        // after the graph's lock.
        drop(built);
        let id = queued.map_err(|refused| match refused {
            QueueRefused::Prepare(error) => batch_refusal(error, failure),
            QueueRefused::Queue(error) => RpcError::from(error),
        })?;
        Ok(status_of(graph, id).expect("an edit just queued has a status"))
    }

    /// Where timed request `id` stands, while the render remembers it (the
    /// latest few dozen of the invention loaded), once what the last
    /// renders settled is received.
    pub fn request_status(&self, id: &str) -> Option<RequestStatus> {
        self.settle_timed_writes();
        status_of(self.graph.as_ref()?, parse_id(id)?)
    }

    /// Every timed request whose status changed since the last call, at its
    /// latest, in the order they last changed: a replaced invention's
    /// first, its last state the one it was left in.
    pub fn take_request_changes(&self) -> Vec<RequestStatus> {
        self.settle_timed_writes();
        let mut changes: Vec<_> = self.edit_changes.0.lock().unwrap().drain(..).collect();
        changes.extend(self.graph.as_ref().map(ledger_changes).unwrap_or_default());
        changes
    }

    /// Keeps the changes of the invention about to be replaced, its timed
    /// edit settled, for [`Self::take_request_changes`].
    pub(super) fn keep_request_changes(&self) {
        let Some(graph) = &self.graph else {
            return;
        };
        let mut kept = self.edit_changes.0.lock().unwrap();
        for change in ledger_changes(graph) {
            if kept.len() == LEDGER_CAPACITY {
                kept.pop_front();
            }
            kept.push_back(change);
        }
    }
}

/// The last write `candidate`, the batch `edits` checked, makes to each
/// control of a module it keeps (none it adds or swaps in), a sample's
/// path resolved, as the daemon chains them to its landing.
fn kept_writes(
    edits: &[StructuralEdit],
    candidate: &EditedCandidate,
) -> Vec<(String, String, ControlValue)> {
    let built: HashSet<&str> = (edits.iter())
        .filter_map(|edit| builds_module(edit).map(|(id, ..)| id))
        .collect();
    let source_path = candidate.document.source_path.as_deref();
    (candidate.final_writes().into_iter())
        .filter(|candidate| !built.contains(candidate.write.module_id.as_str()))
        .map(|candidate| {
            let write = &candidate.write;
            let asset = (write.key.as_str(), candidate.asset);
            let load = audio_assets::resolve_checked_asset_write(source_path, asset, &write.value);
            (write.module_id.clone(), write.key.clone(), load)
        })
        .collect()
}

/// Timed request `id`'s status on the render `graph`.
fn status_of(graph: &std::sync::Mutex<SignalGraph>, id: RequestId) -> Option<RequestStatus> {
    let locked = graph.lock().unwrap();
    let status = locked.offline.as_ref()?.edits.ledger.status(id);
    status.map(wire_status)
}

/// What changed on the render `graph`'s timed edits since last taken.
fn ledger_changes(graph: &std::sync::Arc<std::sync::Mutex<SignalGraph>>) -> Vec<RequestStatus> {
    let mut locked = graph.lock().unwrap();
    let Some(offline) = locked.offline.as_mut() else {
        return Vec::new();
    };
    let changes = offline.edits.ledger.take_changes();
    changes.into_iter().map(wire_status).collect()
}
