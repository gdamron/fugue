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
//!   audio thread at its sample.
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
//! while taking the publisher. It peeks at the route, takes the publisher,
//! then checks the route again under it, so a module retired meanwhile
//! (which happens under the publisher, as its replacement commits) refuses
//! the write rather than letting it reach the module that replaced it.

use std::sync::{Arc, Mutex, Weak};

use super::publish::Publisher;
use crate::control_request::{
    apply_declared, ControlCells, ControlIndex, ControlTable, Request, RequestSender, RequestValue,
    RtValue,
};
use crate::traits::ControlSurfaceMap;
use crate::{ControlMeta, ControlSurface, ControlValue, Module};

/// Where a declared surface's writes go (see the module docs).
#[derive(Clone)]
pub(crate) enum Route {
    /// The controls written so far, each once.
    Building(Vec<ControlIndex>),
    Live(RequestPort),
    Prepared,
    Retired,
}

/// A live graph's request queue, as one module's surface submits to it.
#[derive(Clone)]
pub(crate) struct RequestPort {
    pub(crate) publisher: Weak<Mutex<Publisher>>,
    pub(crate) requests: RequestSender,
    pub(crate) module_id: String,
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
                port.requests
                    .submit(request)
                    .map_err(|_| "The control request queue is full; try again")?;
                publisher.note_written();
                Ok(())
            }
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
}
