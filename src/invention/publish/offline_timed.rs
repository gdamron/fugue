//! A structural edit timed on an offline render: prepared and checked now,
//! against the graph that is playing, as an untimed edit is (see
//! `change::offline`), then queued through the render's own request queue
//! with a `when`, so it lands where a live graph's timed edit lands (see
//! `timed`, `graph::offline_edits`), by the same rule and the same drain.
//! What it changes in the runtime's mirrors and document is committed as
//! it is adopted: as the render's next control call settles it, once it
//! has landed.

use std::sync::{Arc, Mutex, Weak};

use indexmap::IndexMap;

use super::change::{commit_mirrors, prepare_offline, OfflinePrepared};
use super::offline_chain::{resolve_offline_chain, submit_offline_chain, OfflineChained};
use super::rebind::Packed;
use super::timed::{edit_when, refuse_followed};
use super::{CancelCause, EditTime, GraphChange, PreparedChange, TopologyMirror, Touched};
use crate::control_request::{QueueFull, Request, RequestId, When};
use crate::invention::graph::ended::EndGates;
use crate::invention::graph::{Fate, SignalGraph, WaitingEdit, TIMED_EDIT_MODULE};
use crate::invention::orchestration::ModulePorts;
use crate::invention::request_log::LoggedBeat;
use crate::invention::runtime::{ControlSurfaceInstance, GraphCommandError};
use crate::invention::state::RuntimeState;
use crate::modules::control_scheduler::SurfaceDirectory;
use crate::payload::Payload;
use crate::ControlValue;
use crate::InventionHandles;

/// The runtime mirrors a timed offline edit commits to as it is adopted:
/// the control-surface directory, module ports and runtime state, and the
/// handles of the modules it builds.
pub(crate) type OfflineTargets<'a> = (
    &'a SurfaceDirectory,
    &'a Arc<Mutex<IndexMap<String, ModulePorts>>>,
    &'a Arc<Mutex<RuntimeState>>,
    &'a Arc<Mutex<InventionHandles>>,
);

/// What adopting a timed offline edit commits, and where: the mirrors are
/// held weakly, so an edit waiting keeps no render's runtime alive.
pub(crate) struct OfflineAdoption {
    /// The change, its publication queued.
    prepared: PreparedChange,
    /// The modules running as it was prepared, in order.
    previous: Vec<String>,
    /// The schedules it hands surviving schedulers, recorded once adopted,
    /// and what a schedule write would race (see [`Self::races`]).
    packed: Packed,
    surfaces: Weak<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
    ports: Weak<Mutex<IndexMap<String, ModulePorts>>>,
    state: Weak<Mutex<RuntimeState>>,
    handles: Weak<Mutex<InventionHandles>>,
}

impl OfflineAdoption {
    /// What the edit does to module `module_id` as it lands, if it removes
    /// or rebuilds it.
    pub(crate) fn touches(&self, module_id: &str) -> Option<Touched> {
        if !self.previous.iter().any(|id| id == module_id) {
            return None;
        }
        if self.prepared.built.contains_key(module_id) {
            Some(Touched::Rebuilt)
        } else if !self.prepared.mirror.modules.contains_key(module_id) {
            Some(Touched::Removed)
        } else {
            None
        }
    }

    /// Whether a schedule write to `module_id`, resolved against the graph
    /// playing to write `targets`, races the edit, as on a live graph (see
    /// `Publisher::timed_races`).
    pub(crate) fn races(&self, module_id: &str, targets: &[String]) -> bool {
        self.packed.races(module_id, targets)
    }
}

/// Why a timed edit on an offline render was refused, with nothing queued:
/// as preparing it refused it, as an untimed edit would be, or as queueing
/// it did.
pub(crate) enum QueueRefused {
    Prepare(GraphCommandError),
    Queue(GraphCommandError),
}

/// Queues the change `edit` makes to an offline render's `graph` to land
/// at `time`, refused once `ttl` samples pass first, and returns its
/// request's id. Its `writes` (module, control, value) to modules it keeps
/// play from the sample it lands on (see `offline_chain`). It is adopted
/// into `targets` once it lands (see [`adopt_offline`]). Build its modules
/// before calling, so builds stay outside the graph's lock.
///
/// Refused with nothing queued as an untimed edit is (`Prepare`), and as a
/// live graph's timed edit is (`Queue`): when it adds or removes a code or
/// agent module, `time` names a clock or module that is not there, a write
/// cannot be timed or does not fit its control, another timed edit waits,
/// it changes no structure (`ControlError`), the queue is full, or a timed
/// edit the render landed has yet to be adopted (`TopologyMoved`). What a refused change built drops off the lock.
pub(crate) fn queue_offline(
    graph: &Arc<Mutex<SignalGraph>>,
    targets: OfflineTargets<'_>,
    time: EditTime<'_>,
    ttl: Option<u64>,
    writes: &[(String, String, ControlValue)],
    edit: impl FnOnce(&mut GraphChange) -> Result<(), GraphCommandError>,
) -> Result<RequestId, QueueRefused> {
    let (surfaces, _, state, _) = targets;
    let mut locked = graph.lock().unwrap();
    // Not retried: its one caller, the render engine, settles first, and
    // renders on the same thread, so none lands in between.
    if locked.offline_edit_landed() {
        drop(locked);
        return Err(QueueRefused::Queue(GraphCommandError::TopologyMoved));
    }
    let running = TopologyMirror::of(&locked.modules, &locked.edges);
    let (prepared, discarded) = prepare_offline(&mut locked, surfaces, edit);
    let mut prepared = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            drop(locked);
            drop(discarded);
            return Err(QueueRefused::Prepare(error));
        }
    };
    let mut unsent = None;
    // In the order a live graph's publisher refuses it (see
    // `LiveGraph::commit_built_when`).
    let followed = |prepared: &OfflinePrepared| {
        refuse_followed(&state.lock().unwrap(), &running, &prepared.prepared)
    };
    let queued = (prepared.as_ref().map_or(Ok(()), followed))
        .and_then(|()| edit_when(&running, time))
        .and_then(|resolved| Ok((resolved, resolve_offline_chain(&locked, surfaces, writes)?)))
        .and_then(|(resolved, chain)| {
            refuse_timed(&locked, prepared.is_some())?;
            let parts = (time, resolved, ttl);
            submit(
                &mut locked,
                &mut prepared,
                (parts, chain),
                targets,
                &mut unsent,
            )
        });
    drop(locked);
    drop((prepared, unsent, discarded));
    queued.map_err(QueueRefused::Queue)
}

/// Refuses timing a change, as a live graph's publisher does, when another
/// timed edit waits on `locked`, or it changes no structure (`!changes`).
fn refuse_timed(locked: &SignalGraph, changes: bool) -> Result<(), GraphCommandError> {
    let control = |message: &str| Err(GraphCommandError::ControlError(message.into()));
    let waits = (locked.offline.as_ref()).is_some_and(|offline| offline.edits.waiting().is_some());
    match changes {
        _ if waits => control("A timed edit already waits; try again once it lands"),
        false => control("Only a structural edit can be timed"),
        true => Ok(()),
    }
}

/// When a timed edit lands, how long it may wait, and what it resolved to.
type TimedParts<'a> = (EditTime<'a>, (When, Option<LoggedBeat>), Option<u64>);

/// Submits `prepared` as a timed edit request on `locked`'s queue, after
/// its `chain` of writes waiting on it, and waits on it. Refused, it is
/// left where it is, and a request the queue hands back in `unsent`, to
/// drop off the lock.
fn submit(
    locked: &mut SignalGraph,
    prepared: &mut Option<OfflinePrepared>,
    ((time, (when, beat), ttl), chain): (TimedParts<'_>, Vec<OfflineChained>),
    (surfaces, ports, state, handles): OfflineTargets<'_>,
    unsent: &mut Option<(Request, PreparedChange, Packed)>,
) -> Result<RequestId, GraphCommandError> {
    let generation = (locked.request_generation(), locked.offline_edits + 1);
    let Some(offline) = locked.offline.as_mut() else {
        return Err(GraphCommandError::AudioThreadStopped);
    };
    if !offline.sender.has_room_for(chain.len() as u64 + 1) {
        return Err(GraphCommandError::QueueFull);
    }
    let id = offline.sender.take_id();
    // Queued ahead of the edit, so the render has them waiting as it lands.
    let chained = submit_offline_chain(locked, id, chain)?;
    let offline = locked.offline.as_mut().expect("submitted to");
    let OfflinePrepared {
        mut publication,
        prepared,
        packed,
        previous,
    } = prepared.take().expect("a structural change");
    let fate = Fate::default();
    publication.generation = generation.1;
    publication.fate = Some(fate.guard());
    let outputs =
        (prepared.mirror.modules.iter()).map(|(id, module)| (id, &module.ports.outputs[..]));
    let gates = EndGates::of(outputs);
    let mut request = Request::edit(generation.0, Payload::owned(publication));
    request.target.module_idx = TIMED_EDIT_MODULE;
    request.when = when;
    request.ttl = ttl;
    // The queue had room under the graph's lock, which every submission
    // to it takes.
    if let Err(QueueFull(request)) = offline.sender.submit_as(id, request) {
        offline.sender.cancelled(id);
        *unsent = Some((request, prepared, packed));
        return Err(GraphCommandError::QueueFull);
    }
    let adoption = OfflineAdoption {
        prepared,
        previous,
        packed,
        surfaces: Arc::downgrade(surfaces),
        ports: Arc::downgrade(ports),
        state: Arc::downgrade(state),
        handles: Arc::downgrade(handles),
    };
    let sample = match time {
        EditTime::AtSample(sample) => Some(sample),
        _ => None,
    };
    let replaces = generation.0;
    let edit = WaitingEdit {
        id,
        fate,
        adoption,
        replaces,
        chained,
    };
    offline.edits.queued(edit, gates, (sample, beat));
    Ok(id)
}

/// Adopts `landed`, a timed edit an offline render's `graph` landed:
/// commits the runtime's mirrors and document to it, as an untimed edit
/// commits as it installs, records the schedules it handed over, takes
/// its built modules' handles, and records it adopted. Returns the modules
/// it stopped. Control thread, off the graph's lock.
pub(crate) fn adopt_offline(graph: &Arc<Mutex<SignalGraph>>, landed: WaitingEdit) -> Vec<String> {
    let WaitingEdit { id, adoption, .. } = landed;
    let OfflineAdoption {
        prepared,
        previous,
        packed,
        surfaces,
        ports,
        state,
        handles,
    } = adoption;
    let (Some(surfaces), Some(ports), Some(state)) =
        (surfaces.upgrade(), ports.upgrade(), state.upgrade())
    else {
        return Vec::new();
    };
    // Under the graph's lock, as an untimed edit commits (see
    // `edit_offline`), so a write checking its route under it reaches the
    // module its surface belongs to.
    let mut locked = graph.lock().unwrap();
    for schedule in packed.written {
        schedule.record();
    }
    let mut committed = commit_mirrors(graph, &previous, prepared, (&surfaces, &ports, &state));
    let built = std::mem::take(&mut committed.handles);
    if let Some(handles) = handles.upgrade() {
        let mut handles = handles.lock().unwrap();
        for id in &committed.stopped {
            handles.remove_prefix(&format!("{id}."));
        }
        handles.merge(InventionHandles::new(built));
    }
    let stopped = committed.stopped.clone();
    if let Some(offline) = locked.offline.as_mut() {
        offline.edits.ledger.adopted(id, committed);
    }
    stopped
}

/// Cancels the timed edit waiting on an offline render's `graph` when it
/// rebuilds or removes `module_id`, whose control `key` an authored write
/// just set, or writes that control as it lands, unless the write is
/// `spared` (kept over it, or lost): the write is kept, as on a live graph.
/// Control thread.
pub(crate) fn reconcile_offline(
    graph: &Mutex<SignalGraph>,
    (module_id, key): (&str, &str),
    spared: bool,
) {
    let mut locked = graph.lock().unwrap();
    let Some(offline) = locked.offline.as_mut() else {
        return;
    };
    offline.settle();
    let chains = |edit: &WaitingEdit| {
        let chained = |(module, control): &(String, String)| module == module_id && control == key;
        (!spared && edit.chained.iter().any(chained)).then_some(Touched::Written)
    };
    let touches = (offline.edits.waiting())
        .and_then(|edit| edit.adoption.touches(module_id).or_else(|| chains(edit)));
    if let Some(touches) = touches {
        offline.cancel_edit(|| CancelCause::AuthoredWrite {
            module_id: module_id.to_string(),
            key: key.to_string(),
            touches,
        });
    }
}
