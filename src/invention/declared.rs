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
//!   is bound to run (what it was built with stands for the rest). An event
//!   written so (a development restoring its saved config) fires then, once,
//!   with the value last written, in write order; for a module that queues
//!   its events (an instrument's notes) every one written fires, in order.
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
//! to: applied, never pending. A payload control is the exception: no cell
//! holds it, so reads report the last write the surface accepted (built,
//! staged, submitted or applied offline), held here, as the old
//! setters did: a live write refused after it was submitted (its module
//! replaced meanwhile, the pending store full at an install) still reads
//! back until the next write.
//!
//! # Locks
//!
//! A surface's route lock is innermost but for its payloads lock, which
//! is never held while taking another: a write never holds the route lock
//! while taking the publisher or an offline graph's lock. It peeks at the
//! route, takes that lock, then checks the route again under it, so a
//! module retired meanwhile (which happens under that same lock, as its
//! replacement commits or installs) refuses the write rather than letting
//! it reach the module that replaced it.

use std::any::Any;
use std::sync::{Arc, Mutex, Weak};

use super::graph::SignalGraph;
use super::publish::{PendingLog, Publisher, Reclaimer, Retarget, Settler};
use crate::control_request::{
    ControlCells, ControlContext, ControlDecl, ControlIndex, ControlTable, Rank, RequestSender,
    RtValue,
};
use crate::payload::Payload;
#[cfg(doc)]
use crate::ControlSurface;
use crate::ControlValue;

/// A write made while its module builds, applied as the module binds.
#[derive(Clone, Copy)]
pub(crate) enum Staged {
    /// A control, as its cell (or staged payload) holds it then.
    Control(ControlIndex),
    /// One event of a module that queues its events, as written.
    Event(ControlIndex, RtValue),
}

/// Where a declared surface's writes go (see the module docs).
#[derive(Clone)]
pub(crate) enum Route {
    /// The controls written so far, each once, in the order of its last
    /// write: a payload rewriting controls (a melody's whole scale) applies
    /// after those written before it and before those written after. An
    /// event of a module that queues its events is kept at each write.
    Building(Vec<Staged>),
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
    /// Whether its module plays every event written, in order (see
    /// [`ControlCells::queues_events`]).
    pub(crate) queued: bool,
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
    /// Recompiles the topology ahead of a write changing what its module
    /// writes (see [`Checked::targets`]).
    pub(crate) retarget: Retarget,
    /// Frees what the audio side retired before each submission, when no
    /// reclaimer thread does (see [`Reclaimer::reclaim_unthreaded`]).
    pub(crate) reclaimer: Arc<Reclaimer>,
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
    /// Whether a write before its module runs reads back clamped, as the
    /// module will hold it; not a development's, whose aliases each clamp
    /// it for themselves.
    clamps: bool,
    /// Each payload control written so far. The innermost lock: never held
    /// while taking another.
    payloads: Mutex<Vec<Written>>,
    /// What its codecs build with, attached by its module at build (see
    /// [`Self::with_context`]).
    context: Vec<Arc<dyn Any + Send + Sync>>,
}

/// A payload control's last write: what reads report, and while its module
/// is building, the payload waiting to be handed to it.
struct Written {
    index: ControlIndex,
    shown: ControlValue,
    staged: Option<Payload>,
}

#[cfg(test)]
thread_local! {
    /// See [`DeclaredSurface::admitting_all`].
    static ADMIT_ALL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

mod deliver;
mod offline;
mod options;
mod payload;
mod surface;

#[cfg(test)]
pub(crate) use offline::{add_offline, remove_offline};
pub(crate) use offline::{bind_offline, offline_route, retire_offline};
use options::Write;
pub(crate) use options::{Checked, Queued, Resolved, WriteOptions, STALE};

#[cfg_attr(not(test), allow(dead_code))]
impl DeclaredSurface {
    /// A surface over `cells`, which the module built from `table` holds
    /// too, routing writes into the cells until the module is bound.
    pub(crate) fn new(table: ControlTable, cells: Arc<ControlCells>) -> Self {
        Self {
            table,
            cells,
            route: Mutex::new(Route::Building(Vec::new())),
            clamps: true,
            payloads: Mutex::new(Vec::new()),
            context: Vec::new(),
        }
    }

    /// Adds `context` to what its payload codecs build each write with (see
    /// [`ControlContext`]): a part of what its module was built with that a
    /// write needs, such as an asset control's sample rate and build lock.
    /// A codec finds a part by its type, so a module attaches one part per
    /// need. Only this surface can then build its payloads, so a
    /// development aliasing one has it built here
    /// ([`ControlSurface::prepare_payload`]).
    pub(crate) fn with_context(mut self, context: Arc<dyn Any + Send + Sync>) -> Self {
        self.context.push(context);
        self
    }

    /// [`ControlSurface::set_control`] as `options` say, saying what the
    /// write saw as it was queued. An authored write to a live module is
    /// kept over a timed edit waiting that writes the control as it lands
    /// (see `Publisher::keep_authored`). Control thread.
    pub(crate) fn set_control_queued(
        &self,
        key: &str,
        value: ControlValue,
        options: &WriteOptions,
    ) -> Result<Queued, String> {
        if let Some(prepared) = self.prepare(key, &value) {
            let (index, payload, shown) = prepared?;
            return self.deliver(index, Write::Payload(payload, shown, None), options, None);
        }
        let index = self.index(key)?;
        let value = self.coerce(index, &value)?;
        let authored = options.authored.is_some().then_some(key);
        self.deliver(index, Write::Value(value), options, authored)
    }

    /// The live graph's publisher generation, read under its lock (so
    /// after any graph change has updated the directory), or the count of
    /// edits an offline render installed, read under its graph's lock;
    /// `None` when the module is neither. Control thread.
    pub(crate) fn generation(&self) -> Option<u64> {
        // The route lock is released first: a write takes it under the
        // publisher, or under the offline graph's lock.
        let (publisher, graph) = match &*self.route.lock().unwrap() {
            Route::Live(port) => (Some(port.publisher.clone()), None),
            Route::Offline { graph, .. } => (None, Some(graph.clone())),
            _ => return None,
        };
        if let Some(graph) = graph {
            return Some(graph.upgrade()?.lock().unwrap().offline_edits);
        }
        let publisher = publisher?.upgrade()?;
        let generation = publisher.lock().unwrap().generation();
        Some(generation)
    }

    /// Resolves a write of `value` to `key` for a request the live graph
    /// times itself (see `publish::watch`): the control, the value as the
    /// module receives it, whether it is an event, and the key and value
    /// reads list it pending as. Refuses payloads, which a condition
    /// cannot time yet.
    pub(crate) fn resolve_write(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Result<Resolved, String> {
        if self.is_payload(key) {
            return Err(format!("{key} cannot be timed by a condition"));
        }
        let index = self.index(key)?;
        let raw = self.coerce(index, value)?;
        let decl = self.table.decl(index).map(|(decl, _)| decl);
        let (event, reserved) = decl.map_or((false, false), |decl| (decl.event, decl.reserved));
        let shown = self.table.key(index).zip(self.table.value(index, raw));
        Ok(Resolved {
            index,
            raw,
            event,
            reserved,
            shown,
        })
    }

    /// What a write to `index` is built or read with: its position, and
    /// the parts its module attached.
    pub(super) fn context(&self, index: ControlIndex) -> ControlContext<'_> {
        ControlContext {
            position: self.table.decl(index).map_or(0, |(_, position)| position),
            parts: &self.context,
        }
    }

    /// `value` as `index` holds it: read by its module when it declares a
    /// reader (see [`crate::control_request::ValueReader`]), else by its
    /// kind.
    pub(super) fn coerce(
        &self,
        index: ControlIndex,
        value: &ControlValue,
    ) -> Result<RtValue, String> {
        match self.table.decl(index).and_then(|(decl, _)| decl.reader) {
            Some(reader) => (reader.read)(&self.context(index), value),
            None => self.table.coerce(index, value),
        }
    }

    /// A development's surface over `cells`: a write before it runs is
    /// held as written, for each alias to clamp.
    pub(crate) fn fanning_out(table: ControlTable, cells: Arc<ControlCells>) -> Self {
        Self {
            clamps: false,
            ..Self::new(table, cells)
        }
    }

    /// Whether its module has yet to run: reads then come from its cells
    /// alone (see the development's surface).
    pub(crate) fn is_building(&self) -> bool {
        matches!(*self.route.lock().unwrap(), Route::Building(_))
    }

    /// Takes the locks a control thread writing it takes, in its order,
    /// holding them while the guards live.
    #[cfg(test)]
    pub(crate) fn hold_state(&self) -> impl Sized + '_ {
        (self.route.lock().unwrap(), self.payloads.lock().unwrap())
    }

    /// Whether a write to `key` as `options` say would take its control
    /// now: false only offline, when a higher-ranked write holds it at this
    /// sample (see `control_request::hold`). A front door with effects of
    /// its own (a development's alias records and undeclared aliases) asks
    /// first, under a lock its writes to the control all take, so a write
    /// that loses has none. Live, the audio thread weighs the write, and
    /// its outcome says.
    ///
    /// A request log's replay may still take the control between the two
    /// (it holds no front door's lock), so a write admitted here can yet
    /// lose as it is applied, and says so (`Queued::lost`).
    pub(crate) fn admits(&self, key: &str, options: &WriteOptions) -> bool {
        #[cfg(test)]
        if ADMIT_ALL.get() {
            return true;
        }
        let Ok(index) = self.index(key) else {
            return true;
        };
        // The route lock is released before the graph's is taken.
        let route = self.route.lock().unwrap().clone();
        let Route::Offline { graph, module_id } = route else {
            return true;
        };
        let Some(graph) = graph.upgrade() else {
            return true;
        };
        let rank = Rank::new(options.source, options.priority);
        let admits = graph.lock().unwrap().admits(&module_id, index, rank);
        admits
    }

    /// Runs `body` with every write admitted before it is applied, as one
    /// that replay overtakes between the two is.
    #[cfg(test)]
    pub(crate) fn admitting_all<T>(body: impl FnOnce() -> T) -> T {
        /// Restores what was set before, should `body` panic too.
        struct Restore(bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                ADMIT_ALL.set(self.0);
            }
        }
        let _restore = Restore(ADMIT_ALL.replace(true));
        body()
    }

    /// `value`, held by control `key`, as a write to it would carry it: a
    /// choice by name. `None` for a payload, or a key not declared.
    pub(crate) fn value_of(&self, key: &str, value: RtValue) -> Option<ControlValue> {
        self.table.value(self.table.resolve(key)?, value)
    }

    /// The control `key` names, refused when it does not exist: unknown,
    /// or past the read-only count it is listed below.
    pub(super) fn index(&self, key: &str) -> Result<ControlIndex, String> {
        self.table
            .resolve_present(key, |index| self.cells.load(index))
    }
}
