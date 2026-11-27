//! Committing a [`GraphChange`] to an offline render's graph in place.
//!
//! An offline render edits through the same change a live graph does: an
//! upsert drops connections through ports the new module lacks, surviving
//! schedulers whose targets the change rebuilds re-resolve against the new
//! instances (see [`super::rebind::Rescheduling`]), and a change removing a module a
//! schedule still targets, without writing a schedule that no longer does,
//! is refused with nothing changed. The change compiles the same
//! publication, with the same handovers (see `publish::rebind`), and the
//! graph installs it as the audio thread installs a live one: survivors
//! keep their feedback carry and unconnected inputs, a rebuilt module may
//! take over from the instance it replaces. Only the transport differs:
//! the publication is installed at once, under the lock the render takes,
//! rather than queued for an audio thread.

use indexmap::IndexMap;
use std::sync::{Arc, Mutex, MutexGuard};

use super::{BuiltModule, GraphChange, PreparedChange, TopologyMirror};
use crate::invention::declared::offline_route;
use crate::invention::graph::{Publication, SignalGraph};
use crate::invention::orchestration::ModulePorts;
use crate::invention::publish::rebind::{self, Packed};
use crate::invention::publish::{connection_info, Committed};
use crate::invention::runtime::GraphCommandError;
use crate::invention::state::RuntimeState;
use crate::modules::control_scheduler::SurfaceDirectory;

/// The runtime mirrors an offline edit commits with the graph: the
/// control-surface directory, module ports and runtime state.
pub(crate) type OfflineMirrors<'a> = (
    &'a SurfaceDirectory,
    &'a Mutex<IndexMap<String, ModulePorts>>,
    &'a Mutex<RuntimeState>,
);

/// Applies `edit` to an offline render's `graph` as one change, and commits
/// `mirrors` with it, all under the graph's lock, so concurrent edits commit
/// in one order and a write checking its route under the same lock always
/// reaches the module its surface belongs to. `edit` takes the same
/// [`GraphChange`] a live edit does (see `LiveGraph::edit`); build modules
/// before calling, so builds stay outside the lock.
///
/// Fails with nothing changed when `edit` fails, a scheduler it adds cannot
/// resolve its schedule, a surviving scheduler no longer resolves, or the
/// render has been replaced. It installs as an immediate edit does, or as
/// a timed one lands (`landing`; see `SignalGraph::install_offline`).
/// `settle` receives what the render settled, adopting a timed edit it
/// landed.
pub(crate) fn edit_offline(
    graph: &Arc<Mutex<SignalGraph>>,
    mirrors: OfflineMirrors<'_>,
    (landing, settle): (bool, &dyn Fn()),
    edit: impl FnOnce(&mut GraphChange) -> Result<(), GraphCommandError>,
) -> Result<Committed, GraphCommandError> {
    let (directory, module_ports, state) = mirrors;
    let mut locked = lock_adopted(graph, settle)?;
    let (prepared, discarded) = prepare_offline(&mut locked, directory, edit);
    let prepared = match prepared {
        Ok(Some(prepared)) => prepared,
        Ok(None) => return Ok(Committed::default()),
        Err(error) => {
            drop(locked);
            drop(discarded);
            return Err(error);
        }
    };
    let OfflinePrepared {
        publication,
        prepared,
        packed,
        previous,
    } = prepared;
    let publication = locked.install_offline(publication, landing);
    for schedule in packed.written {
        schedule.record();
    }
    let committed = commit_mirrors(graph, &previous, prepared, (directory, module_ports, state));
    drop(locked);
    // What the change displaced drops off the lock.
    drop(publication);
    drop(discarded);
    Ok(committed)
}

/// How many times [`lock_adopted`] settles before refusing: a render on
/// another thread lands at most one edit between two settles.
const ADOPT_ATTEMPTS: usize = 3;

/// Locks `graph` with no timed edit it landed left to adopt (see
/// `publish::offline_timed`): a change made before would be committed
/// under it. `settle` adopts one, off the lock; a render on another thread
/// may land another meanwhile, so it tries a few times.
fn lock_adopted<'a>(
    graph: &'a Mutex<SignalGraph>,
    settle: &dyn Fn(),
) -> Result<MutexGuard<'a, SignalGraph>, GraphCommandError> {
    for attempt in 0..=ADOPT_ATTEMPTS {
        let mut locked = graph.lock().unwrap();
        if !locked.offline_edit_landed() {
            return Ok(locked);
        }
        drop(locked);
        if attempt < ADOPT_ATTEMPTS {
            settle();
        }
    }
    Err(GraphCommandError::TopologyMoved)
}

/// A change prepared against an offline render's graph: compiled, its
/// handovers packed and its survivors mapped onto the modules running, to
/// install in place or queue as a timed edit (see `publish::offline_timed`).
pub(crate) struct OfflinePrepared {
    pub(crate) publication: Box<Publication>,
    /// The change, its publication taken.
    pub(crate) prepared: PreparedChange,
    /// The schedules it hands surviving schedulers, to record once it is
    /// committed.
    pub(crate) packed: Packed,
    /// The modules running as it was prepared, in order.
    pub(crate) previous: Vec<String>,
}

/// Prepares `edit` against `locked`, an offline render's graph, as
/// [`edit_offline`] makes it: `None` when it edits nothing. Returns with it
/// what the change built and set aside, to drop off the graph's lock.
pub(crate) fn prepare_offline(
    locked: &mut SignalGraph,
    directory: &SurfaceDirectory,
    edit: impl FnOnce(&mut GraphChange) -> Result<(), GraphCommandError>,
) -> (
    Result<Option<OfflinePrepared>, GraphCommandError>,
    Vec<BuiltModule>,
) {
    let mut mirror = TopologyMirror::of(&locked.modules, &locked.edges);
    mirror.table_ramps = locked.fit_table_ramps();
    let mut change = GraphChange::new(0, mirror, directory.clone(), locked.block_size);
    // A replaced render takes no edit; what `edit` built is dropped.
    let checked = edit(&mut change).and_then(|()| match locked.retired {
        true => Err(GraphCommandError::AudioThreadStopped),
        false => change.attach(),
    });
    let discarded = change.take_discarded();
    let prepared = checked.map(|()| change.compile()).and_then(|mut prepared| {
        // The same handovers a live commit packs (see `rebind::pack`).
        rebind::pack(&mut prepared).map(|packed| (prepared, packed))
    });
    let (mut prepared, packed) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return (Err(error), discarded),
    };
    let Some(mut publication) = prepared.publication.take() else {
        return (Ok(None), discarded);
    };
    publication.map_survivors(locked.modules.keys().map(String::as_str));
    // A render that follows notes taps the new topology, as a live
    // publisher does with each publication.
    if let Some(keys) = &locked.note_keys {
        publication.note_taps = Some(keys.taps(&prepared.mirror));
    }
    let previous = locked.modules.keys().cloned().collect();
    let prepared = OfflinePrepared {
        publication,
        prepared,
        packed,
        previous,
    };
    (Ok(Some(prepared)), discarded)
}

/// Commits the runtime mirrors to `prepared`, now installed in `graph` in
/// place of the modules `previous` lists, as `LiveGraph::commit_mirrors`
/// does for a published change: surfaces of removed and replaced modules
/// retire, built modules' surfaces route to the graph, and the module
/// ports and runtime state follow the new topology.
pub(in crate::invention::publish) fn commit_mirrors(
    graph: &Arc<Mutex<SignalGraph>>,
    previous: &[String],
    prepared: PreparedChange,
    (directory, module_ports, state): OfflineMirrors<'_>,
) -> Committed {
    let PreparedChange { mirror, built, .. } = prepared;
    let removed: Vec<String> = previous
        .iter()
        .filter(|id| !mirror.modules.contains_key(*id))
        .cloned()
        .collect();
    {
        let mut surfaces = directory.lock().unwrap();
        for id in &removed {
            if let Some(surface) = surfaces.shift_remove(id) {
                surface.retire();
            }
        }
        // A replaced module's surface keeps its place, as its module does.
        for (id, module) in &built {
            if let Some(surface) = surfaces.get(id) {
                surface.retire();
            }
            match &module.surface {
                Some(surface) => {
                    surface.activate(offline_route(graph, id));
                    surfaces.insert(id.clone(), surface.clone());
                }
                None => {
                    surfaces.shift_remove(id);
                }
            }
        }
    }
    *module_ports.lock().unwrap() = mirror
        .modules
        .iter()
        .map(|(id, module)| (id.clone(), module.ports.clone()))
        .collect();
    {
        let mut state = state.lock().unwrap();
        for id in &removed {
            state.modules.shift_remove(id);
            state.written_since.forget(id);
            state.document_remove_module(id);
        }
        for (id, module) in &built {
            state.modules.insert(id.clone(), module.info.clone());
            state.written_since.forget(id);
            state.document_upsert_module(id, &module.info.module_type, &module.info.config);
        }
        state.connections = mirror.edges.iter().map(connection_info).collect();
    }
    let mut committed = Committed {
        stopped: removed,
        ..Committed::default()
    };
    for (id, module) in built {
        if previous.contains(&id) {
            committed.stopped.push(id.clone());
        }
        for (name, handle) in module.handles {
            committed.handles.insert(format!("{id}.{name}"), handle);
        }
        committed.started.push(module.info);
    }
    committed
}
