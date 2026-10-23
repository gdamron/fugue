//! Committing a [`GraphChange`] to an offline render's graph in place.
//!
//! An offline render edits through the same change a live graph does: an
//! upsert drops connections through ports the new module lacks, surviving
//! schedulers whose targets the change rebuilds re-resolve against the new
//! instances (see [`Rescheduling`]), and a change removing a module a
//! schedule still targets, without writing a schedule that no longer does,
//! is refused with nothing changed. The change compiles the same
//! publication, with the same handovers (see `publish::rebind`), and the
//! graph installs it as the audio thread installs a live one: survivors
//! keep their feedback carry and unconnected inputs, a rebuilt module may
//! take over from the instance it replaces. Only the transport differs:
//! the publication is installed at once, under the lock the render takes,
//! rather than queued for an audio thread.

use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use super::{GraphChange, PreparedChange, TopologyMirror};
use crate::invention::declared::offline_route;
use crate::invention::graph::SignalGraph;
use crate::invention::orchestration::ModulePorts;
use crate::invention::publish::{connection_info, rebind, Committed};
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
/// render has been replaced.
pub(crate) fn edit_offline(
    graph: &Arc<Mutex<SignalGraph>>,
    mirrors: OfflineMirrors<'_>,
    edit: impl FnOnce(&mut GraphChange) -> Result<(), GraphCommandError>,
) -> Result<Committed, GraphCommandError> {
    let (directory, module_ports, state) = mirrors;
    let mut locked = graph.lock().unwrap();
    let mirror = TopologyMirror::of(&locked.modules, &locked.edges);
    let mut change = GraphChange::new(0, mirror, directory.clone(), locked.block_size);
    // A replaced render takes no edit; what `edit` built is dropped.
    let checked = edit(&mut change).and_then(|()| match locked.retired {
        true => Err(GraphCommandError::AudioThreadStopped),
        false => change.attach(),
    });
    let discarded = change.take_discarded();
    let prepared = checked.map(|()| change.compile()).and_then(|mut prepared| {
        // The same handovers a live commit packs (see `rebind::pack`).
        rebind::pack(&mut prepared).map(|packed| (prepared, packed.written))
    });
    let (mut prepared, written) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            drop(locked);
            drop(discarded);
            return Err(error);
        }
    };
    let Some(mut publication) = prepared.publication.take() else {
        return Ok(Committed::default());
    };
    publication.map_survivors(locked.modules.keys().map(String::as_str));
    let previous: Vec<String> = locked.modules.keys().cloned().collect();
    locked.install_offline(&mut publication);
    for schedule in written {
        schedule.record();
    }
    let committed = commit_mirrors(graph, &previous, prepared, (directory, module_ports, state));
    drop(locked);
    // What the change displaced drops off the lock.
    drop(publication);
    drop(discarded);
    Ok(committed)
}

/// Commits the runtime mirrors to `prepared`, now installed in `graph` in
/// place of the modules `previous` lists, as `LiveGraph::commit_mirrors`
/// does for a published change: surfaces of removed and replaced modules
/// retire, built modules' surfaces route to the graph, and the module
/// ports and runtime state follow the new topology.
fn commit_mirrors(
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
            state.document_remove_module(id);
        }
        for (id, module) in &built {
            state.modules.insert(id.clone(), module.info.clone());
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
