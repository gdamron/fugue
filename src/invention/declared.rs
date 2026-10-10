//! The control surface of a module with declared controls, and the routes
//! its writes take to the module.
//!
//! A control thread never touches a declared module's state. It resolves
//! the key and coerces the value against the module's
//! [`ControlTable`](crate::control_request::ControlTable), then hands the
//! value over by the surface's [`Route`]:
//!
//! - **Building**: the module is not running yet, so the value lands in its
//!   cells, and each control written so is applied to the module when it
//!   is bound to run (what it was built with stands for the rest).
//! - **Live**: a request through the live graph's queue, applied on the
//!   audio thread at its sample (on a `NullBackend`, by a zero-length
//!   block before the write returns).
//! - **Offline**: applied at once in an offline render's graph, under the
//!   lock its renders take.
//! - **Inner**: inside a development, reached only through the development.
//! - **Prepared**: bound to a change not yet committed; writes are refused
//!   until the change publishes the module (then **Live**), or for good if
//!   it never does.
//! - **Retired**: the module was removed or replaced; writes are refused.
//!
//! Reads come from the cells, which hold what each control was last set
//! to: applied, never pending.
//!
//! # Locks
//!
//! A surface's route lock is the innermost lock: a write never holds it
//! while taking the publisher or an offline graph's lock. It peeks at the
//! route, takes that lock, then checks the route again under it, so a
//! module retired meanwhile (which happens under that same lock, as its
//! replacement commits or installs) refuses the write rather than letting
//! it reach the module that replaced it.

use indexmap::IndexMap;
use std::sync::{Arc, Mutex, Weak};

use super::graph::{GraphCommand, SignalGraph};
use super::publish::{PendingLog, PendingWrite, Publisher, Settler};
use super::runtime::{ControlSurfaceInstance, GraphCommandError, ModuleInstance};
use crate::control_request::{
    apply_declared, Automation, ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind,
    Refusal, Request, RequestSender, RequestValue, RtValue, Writer,
};
use crate::traits::ControlSurfaceMap;
use crate::{ControlMeta, ControlSurface, ControlValue, Module};

/// Where a declared surface's writes go (see the module docs).
#[derive(Clone)]
pub(crate) enum Route {
    /// The controls written so far, each once.
    Building(Vec<ControlIndex>),
    Live(RequestPort),
    Offline {
        graph: Weak<Mutex<SignalGraph>>,
        module_id: String,
    },
    Inner,
    Prepared,
    Retired,
}

/// One declared control as a development aliasing it sees it.
pub(crate) struct Declaration {
    pub(crate) index: ControlIndex,
    pub(crate) decl: ControlDecl,
    /// What it holds now.
    pub(crate) current: RtValue,
}

/// A live graph's request queue, as one module's surface submits to it.
#[derive(Clone)]
pub(crate) struct RequestPort {
    pub(crate) publisher: Weak<Mutex<Publisher>>,
    pub(crate) requests: RequestSender,
    pub(crate) pending: Arc<Mutex<PendingLog>>,
    pub(crate) module_id: String,
    /// Settles each request once submitted, when the backend never renders
    /// on its own (see `LiveGraph::link`).
    pub(crate) settler: Option<Settler>,
}

impl RequestPort {
    /// The same queue, for module `module_id`.
    pub(crate) fn to(&self, module_id: &str) -> Route {
        Route::Live(Self {
            module_id: module_id.to_string(),
            ..self.clone()
        })
    }
}

/// The [`ControlSurface`] of a module with declared controls.
// Built by the first modules to declare their controls.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct DeclaredSurface {
    table: ControlTable,
    cells: Arc<ControlCells>,
    route: Mutex<Route>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl DeclaredSurface {
    /// A surface over `cells`, which the module built from `table` holds
    /// too, routing writes into the cells until the module is bound.
    pub(crate) fn new(table: ControlTable, cells: Arc<ControlCells>) -> Self {
        Self {
            table,
            cells,
            route: Mutex::new(Route::Building(Vec::new())),
        }
    }

    /// Whether its module has yet to run: reads then come from its cells
    /// alone (see the development's surface).
    pub(crate) fn is_building(&self) -> bool {
        matches!(*self.route.lock().unwrap(), Route::Building(_))
    }

    fn index(&self, key: &str) -> Result<ControlIndex, String> {
        self.table
            .resolve(key)
            .ok_or_else(|| format!("Unknown control: {key}"))
    }

    fn deliver(&self, index: ControlIndex, value: RtValue) -> Result<(), String> {
        let event = matches!(self.table.decl(index), Some((decl, _)) if decl.event);
        let route = {
            let mut route = self.route.lock().unwrap();
            if let Route::Building(written) = &mut *route {
                // Under the route lock, so a bind sees it or it sees the bind.
                if event {
                    return Err("An event fires only once its module runs".into());
                }
                self.cells.publish(index, value);
                if !written.contains(&index) {
                    written.push(index);
                }
                return Ok(());
            }
            route.clone()
        };
        match route {
            Route::Building(_) => unreachable!("handled under the lock"),
            Route::Live(port) => {
                let publisher = port
                    .publisher
                    .upgrade()
                    .ok_or("The audio thread has stopped")?;
                let mut publisher = publisher.lock().unwrap();
                self.check_still(|route| matches!(route, Route::Live(_)))?;
                // A kept controller can outlive the audio graph: never queue
                // a request nothing will drain.
                if !publisher.audio_alive() {
                    return Err("The audio thread has stopped".into());
                }
                let target = publisher
                    .control_target(&port.module_id, index)
                    .map_err(|error| error.to_string())?;
                let mut request = Request::new(target, RequestValue::Value(value));
                request.event = event;
                // The log is locked before the submission, so its outcome
                // cannot be received before the write is recorded.
                let mut pending = port.pending.lock().unwrap();
                pending.settle();
                let id = port
                    .requests
                    .submit(request)
                    .map_err(|_| "The control request queue is full; try again")?;
                publisher.note_written();
                if let (Some(key), Some(value)) =
                    (self.table.key(index), self.table.value(index, value))
                {
                    let module_id = port.module_id.clone();
                    let write = PendingWrite {
                        module_id,
                        key,
                        value,
                    };
                    pending.submitted(id, write);
                }
                // Settled with no lock held: other writers carry on, and a
                // request one of them queues meanwhile applies in the same
                // block or theirs.
                drop(pending);
                drop(publisher);
                if let Some(settler) = &port.settler {
                    settler.settle();
                }
                Ok(())
            }
            Route::Offline { graph, module_id } => {
                let graph = graph.upgrade().ok_or("The render has been replaced")?;
                let mut graph = graph.lock().unwrap();
                self.check_still(|route| matches!(route, Route::Offline { .. }))?;
                graph
                    .apply_control(&module_id, index, value)
                    .map_err(|refusal| refused(&self.table, index, refusal))
            }
            Route::Inner => Err("This control is set through its development".into()),
            Route::Prepared => Err("This module is being installed; try again".into()),
            Route::Retired => Err("This module has been removed or replaced".into()),
        }
    }

    /// Refuses a write whose module was retired after the route was read.
    fn check_still(&self, live: impl Fn(&Route) -> bool) -> Result<(), String> {
        if live(&self.route.lock().unwrap()) {
            Ok(())
        } else {
            Err("This module has been removed or replaced".into())
        }
    }
}

fn refused(table: &ControlTable, index: ControlIndex, refusal: Refusal) -> String {
    let key = table.key(index).unwrap_or_default();
    match refusal {
        Refusal::Invalid => format!("Control '{key}' cannot hold that value"),
        _ => format!("Control '{key}' was not applied ({refusal:?})"),
    }
}

impl ControlSurface for DeclaredSurface {
    fn controls(&self) -> Vec<ControlMeta> {
        self.table.metas(|index| {
            let value = self.cells.load(index)?;
            self.table.value(index, value)
        })
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        let index = self.index(key)?;
        self.cells
            .load(index)
            .and_then(|value| self.table.value(index, value))
            .ok_or_else(|| format!("Unknown control: {key}"))
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let index = self.index(key)?;
        let value = self.table.coerce(index, &value)?;
        self.deliver(index, value)
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        self.table.coerce(self.index(key)?, value).map(drop)
    }

    fn bind(&self, route: Route, module: &mut dyn Module) {
        let mut current = self.route.lock().unwrap();
        // A retired surface's module was displaced; it never runs again.
        if matches!(*current, Route::Retired) {
            return;
        }
        debug_assert!(
            module
                .declared()
                .is_some_and(|(_, cells)| std::ptr::eq(cells, &*self.cells)),
            "a declared surface binds only its own module"
        );
        if let Route::Building(written) = &*current {
            for index in written {
                if let Some(value) = self.cells.load(*index) {
                    let _ = apply_declared(module, *index, value);
                }
            }
        }
        *current = route;
    }

    fn activate(&self, route: Route) {
        let mut current = self.route.lock().unwrap();
        if matches!(*current, Route::Prepared) {
            *current = route;
        }
    }

    fn retire(&self) {
        *self.route.lock().unwrap() = Route::Retired;
    }

    fn declares(&self, key: &str) -> bool {
        self.table.resolve(key).is_some()
    }

    fn set_legacy(&self, key: &str, _value: ControlValue) -> Result<(), String> {
        self.index(key).map(drop)
    }

    fn declaration(&self, key: &str) -> Option<Declaration> {
        let index = self.table.resolve(key)?;
        Some(Declaration {
            index,
            decl: self.table.decl(index)?.0.clone(),
            current: self.cells.load(index)?,
        })
    }

    fn automation(&self, key: &str) -> Option<Automation> {
        let index = self.table.resolve(key)?;
        let (decl, _) = self.table.decl(index)?;
        let writable = decl.writer == Writer::Parameter && !decl.event;
        let scalar = matches!(
            decl.kind,
            DeclKind::Number { .. } | DeclKind::Integer { .. } | DeclKind::Bool
        );
        (writable && scalar).then(|| Automation {
            cells: self.cells.clone(),
            index,
            kind: decl.kind,
            clamp: decl.clamp,
            aliases: None,
        })
    }
}

/// Binds every surface in `surfaces` to its module in an offline render's
/// `graph` (a render loading its invention).
pub(crate) fn bind_offline(
    graph: &Arc<Mutex<SignalGraph>>,
    surfaces: &Mutex<IndexMap<String, ControlSurfaceInstance>>,
) {
    let mut locked = graph.lock().unwrap();
    for (id, surface) in surfaces.lock().unwrap().iter() {
        if let Some(instance) = locked.modules.get_mut(id) {
            surface.bind(offline(graph, id), instance.module_mut());
        }
    }
}

/// Adds `module` to an offline render's `graph` as `module_id` in one step
/// under the lock its renders take: lists its `surface` in `surfaces`,
/// retires the one it displaces, binds it, and installs the module. So
/// concurrent edits commit in one order, and a write that checks its
/// route under the same lock always reaches the module its surface
/// belongs to.
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
        surface.bind(offline(graph, module_id), module.module_mut());
    }
    locked.apply_command(GraphCommand::AddModule {
        module_id: module_id.to_string(),
        module,
    });
    Ok(())
}

/// Removes `module_id` from an offline render's `graph` and `surfaces` in
/// one step under the lock its renders take, retiring its surface.
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

fn offline(graph: &Arc<Mutex<SignalGraph>>, module_id: &str) -> Route {
    Route::Offline {
        graph: Arc::downgrade(graph),
        module_id: module_id.to_string(),
    }
}
