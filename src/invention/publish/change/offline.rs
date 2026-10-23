//! Committing a [`GraphChange`] to an offline render's graph in place.
//!
//! An offline render edits through the same change a live graph does: an
//! upsert drops connections through ports the new module lacks, surviving
//! schedulers whose targets the change rebuilds re-resolve against the new
//! instances (see [`Rescheduling`]), and a change removing a module a
//! schedule still targets, without writing a schedule that no longer does,
//! is refused with nothing changed. Only the last step differs: instead of
//! compiling a publication for an audio thread to install, the change is
//! applied to the graph under the lock its renders take, so a rebound
//! scheduler adopts its schedule at once, with no handover.

use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use super::{GraphChange, Rescheduling, TopologyMirror};
use crate::invention::declared::offline_route;
use crate::invention::graph::SignalGraph;
use crate::invention::orchestration::ModulePorts;
use crate::invention::publish::{connection_info, Committed};
use crate::invention::runtime::{GraphCommandError, ModuleInstance};
use crate::invention::state::RuntimeState;
use crate::modules::control_scheduler::{Rebind, SurfaceDirectory};

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
    // Edited even on a replaced render, so what `edit` built is retired
    // below rather than dropped with a surface left open.
    let checked = edit(&mut change).and_then(|()| match locked.retired {
        true => Err(GraphCommandError::AudioThreadStopped),
        false => change.attach(),
    });
    let rebinds = checked.and_then(|()| {
        Rescheduling::rebinds(&change.rescheduling, &change.surfaces, &change.built)
    });
    let rebinds = match rebinds {
        Ok(rebinds) => rebinds,
        Err(error) => {
            // A kept surface of a module never installed takes no writes.
            for module in change.built.values() {
                if let Some(surface) = &module.surface {
                    surface.retire();
                }
            }
            drop(locked);
            return Err(error);
        }
    };
    if change.edits == 0 {
        return Ok(Committed::default());
    }
    let mut built = std::mem::take(&mut change.built);
    let mirror = std::mem::take(&mut change.mirror);

    // The surfaces first, as a live commit does: those of removed and
    // replaced modules retire, and each built module's binds to it here.
    let removed: Vec<String> = locked
        .modules
        .keys()
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
        for (id, module) in built.iter_mut() {
            if let Some(surface) = surfaces.get(id) {
                surface.retire();
            }
            let (Some(surface), Some(instance)) = (&module.surface, module.instance.as_mut())
            else {
                surfaces.shift_remove(id);
                continue;
            };
            surface.bind(offline_route(graph, id), instance.module_mut());
            surfaces.insert(id.clone(), surface.clone());
        }
    }

    // The modules in the mirror's order, as a publication installs them.
    let mut previous = std::mem::take(&mut locked.modules);
    let mut modules: IndexMap<String, ModuleInstance> =
        IndexMap::with_capacity(mirror.modules.len());
    for id in mirror.modules.keys() {
        let instance = built
            .get_mut(id)
            .and_then(|module| module.instance.take())
            .or_else(|| previous.shift_remove(id))
            .expect("every module in the mirror is built or kept");
        modules.insert(id.clone(), instance);
    }
    locked.modules = modules;
    locked.sinks = mirror.sinks();
    locked.edges = mirror.edges.clone();
    locked.connected_in_ports.clear();
    locked.published = false;
    locked.topo_dirty = true;

    // Survivors adopt the schedules re-resolved for them, and record them.
    for Rebind {
        id,
        payload,
        written,
    } in rebinds
    {
        let applied = locked.apply_payload(&id, Rebind::CONTROL, payload);
        debug_assert!(applied.is_ok(), "a scheduler takes its own schedule");
        if applied.is_ok() {
            written.record();
        }
    }

    *module_ports.lock().unwrap() = mirror
        .modules
        .iter()
        .map(|(id, module)| (id.clone(), module.ports.clone()))
        .collect();
    let mut committed = Committed::default();
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
    drop(locked);
    // Modules replaced in place stop too, as their successors start.
    committed.stopped = removed;
    for (id, module) in built {
        if previous.contains_key(&id) && !committed.stopped.contains(&id) {
            committed.stopped.push(id.clone());
        }
        for (name, handle) in module.handles {
            committed.handles.insert(format!("{id}.{name}"), handle);
        }
        committed.started.push(module.info);
    }
    // What the change displaced drops off the lock.
    drop(previous);
    drop(change);
    Ok(committed)
}
