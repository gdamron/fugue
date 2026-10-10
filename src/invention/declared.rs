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
//! to: applied, never pending. A payload control is the exception: no cell
//! holds it, so reads report the last write the surface accepted (built,
//! staged, submitted or applied offline), held here, as the legacy
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

use indexmap::IndexMap;
use std::sync::{Arc, Mutex, Weak};

use super::graph::{GraphCommand, SignalGraph};
use super::publish::{PendingLog, PendingWrite, Publisher, Settler};
use super::runtime::{ControlSurfaceInstance, GraphCommandError, ModuleInstance};
use crate::control_request::{
    apply_declared, Automation, ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind,
    PayloadCodec, Refusal, Request, RequestSender, RequestValue, RtValue, Writer,
};
use crate::payload::Payload;
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
    /// Whether a write before its module runs reads back clamped, as the
    /// module will hold it; not a development's, whose aliases each clamp
    /// it for themselves.
    clamps: bool,
    /// Each payload control written so far. The innermost lock: never held
    /// while taking another.
    payloads: Mutex<Vec<Written>>,
}

/// A payload control's last write: what reads report, and while its module
/// is building, the payload waiting to be handed to it.
struct Written {
    index: ControlIndex,
    shown: ControlValue,
    staged: Option<Payload>,
}

/// A write on its way to a module: a value, or a payload and what reads
/// report for it.
enum Write {
    Value(RtValue),
    Payload(Payload, ControlValue),
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
            clamps: true,
            payloads: Mutex::new(Vec::new()),
        }
    }

    /// Records `value` as what payload control `key` reads back as, for
    /// the payload its module was built with. Control thread.
    pub(crate) fn show_payload(&self, key: &str, value: ControlValue) {
        if let Some(index) = self.table.resolve(key) {
            self.show(index, value, None);
        }
    }

    /// Whether `key` is a payload control.
    pub(crate) fn is_payload(&self, key: &str) -> bool {
        self.payload_decl(key).is_some()
    }

    fn payload_decl(&self, key: &str) -> Option<(ControlIndex, Option<PayloadCodec>)> {
        let index = self.table.resolve(key)?;
        let (decl, _) = self.table.decl(index)?;
        (decl.kind == DeclKind::Payload).then_some((index, decl.codec))
    }

    fn shown(&self, index: ControlIndex) -> Option<ControlValue> {
        let payloads = self.payloads.lock().unwrap();
        let written = payloads.iter().find(|written| written.index == index)?;
        Some(written.shown.clone())
    }

    /// Records a payload control's write; `staged` replaces any payload
    /// still waiting for the module (dropped here, on a control thread).
    fn show(&self, index: ControlIndex, shown: ControlValue, staged: Option<Payload>) {
        let mut payloads = self.payloads.lock().unwrap();
        match payloads.iter_mut().find(|written| written.index == index) {
            Some(written) => {
                written.shown = shown;
                written.staged = staged;
            }
            None => payloads.push(Written {
                index,
                shown,
                staged,
            }),
        }
    }

    /// Builds a payload control's write with its codec, or refuses it.
    fn prepare(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Option<Result<(ControlIndex, Payload, ControlValue), String>> {
        let (index, codec) = self.payload_decl(key)?;
        let Some(codec) = codec else {
            return Some(Err(format!("Control '{key}' cannot be written")));
        };
        Some((codec.prepare)(value).map(|(payload, shown)| (index, payload, shown)))
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

    fn index(&self, key: &str) -> Result<ControlIndex, String> {
        self.table
            .resolve(key)
            .ok_or_else(|| format!("Unknown control: {key}"))
    }

    fn deliver(&self, index: ControlIndex, write: Write) -> Result<(), String> {
        let (event, clamp) = match self.table.decl(index) {
            Some((decl, _)) => (decl.event, decl.clamp.filter(|_| self.clamps)),
            None => (false, None),
        };
        let route = {
            let mut route = self.route.lock().unwrap();
            if let Route::Building(written) = &mut *route {
                // Under the route lock, so a bind sees it or it sees the bind.
                if event {
                    return Err("An event fires only once its module runs".into());
                }
                let value = match write {
                    Write::Value(value) => value,
                    Write::Payload(payload, shown) => {
                        self.show(index, shown, Some(payload));
                        return Ok(());
                    }
                };
                // Held as the module will hold it once it applies it.
                let held = match (value, clamp) {
                    (RtValue::F32(number), Some((min, max))) => {
                        RtValue::F32(number.max(min).min(max))
                    }
                    (RtValue::I32(whole), Some((min, max))) => {
                        RtValue::I32(whole.clamp(min as i32, max as i32))
                    }
                    _ => value,
                };
                self.cells.publish(index, held);
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
                let (value, shown) = match write {
                    Write::Value(value) => (RequestValue::Value(value), None),
                    Write::Payload(payload, shown) => (RequestValue::Payload(payload), Some(shown)),
                };
                let read = match (&value, &shown) {
                    (RequestValue::Value(value), _) => self.table.value(index, *value),
                    (_, shown) => shown.clone(),
                };
                let mut request = Request::new(target, value);
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
                // Under the publisher's lock, so reads report the last write
                // submitted.
                if let Some(shown) = shown {
                    self.show(index, shown, None);
                }
                if let (Some(key), Some(value)) = (self.table.key(index), read) {
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
                match write {
                    Write::Value(value) => graph.apply_control(&module_id, index, value),
                    Write::Payload(payload, shown) => {
                        let applied = graph.apply_payload(&module_id, index, payload);
                        if applied.is_ok() {
                            self.show(index, shown, None);
                        }
                        applied
                    }
                }
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
            self.table.value(index, value).or_else(|| self.shown(index))
        })
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        if let Some((index, _)) = self.payload_decl(key) {
            return self
                .shown(index)
                .ok_or_else(|| format!("Control '{key}' holds no value yet"));
        }
        let index = self.index(key)?;
        self.cells
            .load(index)
            .and_then(|value| self.table.value(index, value))
            .ok_or_else(|| format!("Unknown control: {key}"))
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        if let Some(prepared) = self.prepare(key, &value) {
            let (index, payload, shown) = prepared?;
            return self.deliver(index, Write::Payload(payload, shown));
        }
        let index = self.index(key)?;
        let value = self.table.coerce(index, &value)?;
        self.deliver(index, Write::Value(value))
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        if let Some(prepared) = self.prepare(key, value) {
            return prepared.map(drop);
        }
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
            // Payloads written meanwhile; what they replace, or a refused
            // one, is dropped here, on this control thread.
            for written in self.payloads.lock().unwrap().iter_mut() {
                if let Some(payload) = written.staged.take() {
                    let applied = module.apply_payload(written.index, payload);
                    debug_assert!(applied.is_ok(), "a module takes what its codec builds");
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
