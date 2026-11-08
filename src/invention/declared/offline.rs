//! Declared surfaces in an offline render's graph.

use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use super::Route;
#[cfg(test)]
use crate::invention::graph::GraphCommand;
use crate::invention::graph::SignalGraph;
use crate::invention::runtime::ControlSurfaceInstance;
#[cfg(test)]
use crate::invention::runtime::{GraphCommandError, ModuleInstance};

/// Binds every surface in `surfaces` to its module in an offline render's
/// `graph` (a render loading its invention).
pub(crate) fn bind_offline(
    graph: &Arc<Mutex<SignalGraph>>,
    surfaces: &Mutex<IndexMap<String, ControlSurfaceInstance>>,
) {
    let mut locked = graph.lock().unwrap();
    for (id, surface) in surfaces.lock().unwrap().iter() {
        if let Some(instance) = locked.modules.get_mut(id) {
            surface.bind(offline_route(graph, id), instance.module_mut());
        }
    }
    // Bound, they play the schedules they were built with.
    locked.fit_table_ramps();
}

/// Adds a bare `module` to an offline render's `graph` as `module_id` in one
/// step under the lock its renders take: lists its `surface` in `surfaces`,
/// retires the one it displaces, binds it, and installs the module. So
/// concurrent edits commit in one order, and a write that checks its
/// route under the same lock always reaches the module its surface
/// belongs to. A test fixture: renders edit through `publish::edit_offline`,
/// which also keeps their schedules and connections as a live edit does.
#[cfg(test)]
pub(crate) fn add_offline(
    graph: &Arc<Mutex<SignalGraph>>,
    surfaces: &Mutex<IndexMap<String, ControlSurfaceInstance>>,
    module_id: &str,
    mut module: ModuleInstance,
    surface: Option<ControlSurfaceInstance>,
) -> Result<(), GraphCommandError> {
    let mut locked = graph.lock().unwrap();
    if locked.retired {
        if let Some(surface) = surface {
            surface.retire();
        }
        return Err(GraphCommandError::AudioThreadStopped);
    }
    let displaced = {
        let mut surfaces = surfaces.lock().unwrap();
        match &surface {
            Some(surface) => surfaces.insert(module_id.to_string(), surface.clone()),
            None => surfaces.shift_remove(module_id),
        }
    };
    if let Some(displaced) = displaced {
        displaced.retire();
    }
    if let Some(surface) = surface {
        surface.bind(offline_route(graph, module_id), module.module_mut());
    }
    locked.apply_command(GraphCommand::AddModule {
        module_id: module_id.to_string(),
        module,
    });
    Ok(())
}

/// Removes `module_id` from an offline render's `graph` and `surfaces` in
/// one step under the lock its renders take, retiring its surface. A test
/// fixture, as [`add_offline`] is.
#[cfg(test)]
pub(crate) fn remove_offline(
    graph: &Arc<Mutex<SignalGraph>>,
    surfaces: &Mutex<IndexMap<String, ControlSurfaceInstance>>,
    module_id: &str,
) -> Result<(), GraphCommandError> {
    let mut locked = graph.lock().unwrap();
    if locked.retired {
        return Err(GraphCommandError::AudioThreadStopped);
    }
    let removed = surfaces.lock().unwrap().shift_remove(module_id);
    if let Some(removed) = removed {
        removed.retire();
    }
    locked.apply_command(GraphCommand::RemoveModule {
        module_id: module_id.to_string(),
    });
    Ok(())
}

/// Retires `graph` and every surface in `surfaces` under its lock: the
/// render is being replaced, so a write through a kept surface must not
/// land in a graph nobody renders any more, and an edit still in flight
/// through a kept controller is refused rather than add a module there.
pub(crate) fn retire_offline(
    graph: &Arc<Mutex<SignalGraph>>,
    surfaces: &Mutex<IndexMap<String, ControlSurfaceInstance>>,
) {
    let mut locked = graph.lock().unwrap();
    locked.retired = true;
    for surface in surfaces.lock().unwrap().values() {
        surface.retire();
    }
}

/// The route a surface takes to its module in an offline render's `graph`.
pub(crate) fn offline_route(graph: &Arc<Mutex<SignalGraph>>, module_id: &str) -> Route {
    Route::Offline {
        graph: Arc::downgrade(graph),
        module_id: module_id.to_string(),
    }
}
