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
//!   with the value last written, in write order.
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
use std::any::Any;
use std::sync::{Arc, Mutex, Weak};

#[cfg(test)]
use super::graph::GraphCommand;
use super::graph::SignalGraph;
use super::publish::{
    Authoring, CancelCause, PendingLog, PendingWrite, Publisher, Retarget, Settler, Touched,
};
use super::runtime::ControlSurfaceInstance;
#[cfg(test)]
use super::runtime::{GraphCommandError, ModuleInstance};
use crate::control_request::{
    apply_declared, Automation, ControlCells, ControlContext, ControlDecl, ControlIndex,
    ControlTable, DeclKind, Intent, PayloadCodec, Rank, Refusal, Request, RequestId,
    RequestSender, RequestValue, RtValue, Source, Writer,
};
use crate::payload::Payload;
use crate::traits::ControlSurfaceMap;
use crate::{ControlMeta, ControlSurface, ControlValue, Module};

/// Where a declared surface's writes go (see the module docs).
#[derive(Clone)]
pub(crate) enum Route {
    /// The controls written so far, each once, in the order of its last
    /// write: a payload rewriting controls (a melody's whole scale) applies
    /// after those written before it and before those written after.
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
    /// Recompiles the topology ahead of a write changing what its module
    /// writes (see [`Checked::targets`]).
    pub(crate) retarget: Retarget,
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

/// What a write saw as it was queued for the live graph, under the
/// publisher's lock: the timed edit the write is safe from (see
/// `Publisher::keep_authored`), and the request it was submitted as.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Queued {
    pub(crate) safe: Option<RequestId>,
    /// The request the write was submitted as, whose outcome says when it
    /// applied; `None` when it applied as it was made (building, offline,
    /// a legacy control) or travels in a publication, which reports none.
    pub(crate) id: Option<RequestId>,
    /// An offline write that lost to a higher-ranked write at the same
    /// sample, so nothing was applied (see `control_request::hold`).
    pub(crate) lost: bool,
}

/// How a front door submits a write: whether it is authored (and what the
/// retained document records once it applies), who wrote it and its rank
/// within that tier (see `control_request::hold`), how many samples it
/// may wait before it is refused `Expired`, and whether it is announced
/// once it applies (see `PendingWrite::announced`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WriteOptions<'a> {
    pub(crate) authored: Option<&'a Authoring>,
    pub(crate) source: Source,
    pub(crate) priority: i16,
    pub(crate) ttl: Option<u64>,
    pub(crate) announced: bool,
}

impl WriteOptions<'_> {
    /// A performed write from `source` at priority 0, with no ttl.
    pub(crate) const fn performed(source: Source) -> Self {
        Self {
            authored: None,
            source,
            priority: 0,
            ttl: None,
            announced: false,
        }
    }

    /// The same write, authored: recorded as `authoring` says once it
    /// applies.
    pub(crate) const fn authored_as(self, authoring: &Authoring) -> WriteOptions<'_> {
        WriteOptions {
            authored: Some(authoring),
            source: self.source,
            priority: self.priority,
            ttl: self.ttl,
            announced: self.announced,
        }
    }

    /// The same write, announced once it applies.
    pub(crate) const fn announcing(self) -> Self {
        Self {
            announced: true,
            ..self
        }
    }

    /// The same write, performed.
    pub(crate) const fn performing(self) -> Self {
        Self {
            authored: None,
            ..self
        }
    }
}

impl Default for WriteOptions<'_> {
    /// A user's performed write: what a control write without options is.
    fn default() -> Self {
        Self::performed(Source::User)
    }
}

/// A write resolved for a request the live graph times itself.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Resolved {
    pub(crate) index: ControlIndex,
    pub(crate) raw: RtValue,
    pub(crate) event: bool,
    /// Its key and value as reads list it pending.
    pub(crate) shown: Option<(String, ControlValue)>,
}

/// A write on its way to a module: a value, or a payload and what reads
/// report for it, checked as [`Checked`] says when built against the
/// directory.
enum Write<'a> {
    Value(RtValue),
    Payload(Payload, ControlValue, Option<Checked<'a>>),
}

/// A payload write built against the live graph's control-surface
/// directory (see [`DeclaredSurface::write_payload_checked`]).
pub(crate) struct Checked<'a> {
    /// The publisher generation the directory was read at
    /// ([`DeclaredSurface::generation`]); a live write the publisher would
    /// stamp with any other is refused as [`STALE`], since a graph change
    /// may have replaced what the payload resolved; so is an offline write
    /// after an edit installed in place since.
    pub(crate) generation: Option<u64>,
    /// The modules the payload writes: a timed edit waiting that rebuilds
    /// or removes one, or that re-resolved the payload this write
    /// replaces, is cancelled (see `Publisher::timed_races`). On a live
    /// graph, a topology compiled with the module writing others is
    /// recompiled with these and published just ahead of the write (see
    /// `publish::retarget`).
    pub(crate) targets: Vec<String>,
    /// Records the write where it commits: under the publisher on a live
    /// graph, so a graph change sees every write submitted before it
    /// recorded, and none after.
    pub(crate) record: Box<dyn FnOnce() + 'a>,
}

/// A live payload write built against a directory a graph change has
/// replaced since: build it again.
pub(crate) const STALE: &str = "The graph changed while this write was prepared; try again";

const QUEUE_FULL: &str = "The control request queue is full; try again";

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

    /// Records `value` as what payload control `key` reads back as, for
    /// the payload its module was built with, or as another key on the
    /// same inner control wrote it; nothing for a key that names no
    /// control (see [`Self::index`]). A payload still waiting for the
    /// module stays: the bind applies every key's last write in the order
    /// written, so an inner control still takes the later one, and an
    /// alias only the earlier key reaches keeps it. Control thread.
    pub(crate) fn show_payload(&self, key: &str, value: ControlValue) {
        if let Ok(index) = self.index(key) {
            let mut payloads = self.payloads.lock().unwrap();
            match payloads.iter_mut().find(|written| written.index == index) {
                Some(written) => written.shown = value,
                None => payloads.push(Written {
                    index,
                    shown: value,
                    staged: None,
                }),
            }
        }
    }

    /// Writes payload control `key` with a payload built by its module's
    /// own surface rather than a codec (one whose building needs more than
    /// the value written), reading back as `shown`. Control thread.
    pub(crate) fn write_payload(
        &self,
        key: &str,
        payload: Payload,
        shown: ControlValue,
    ) -> Result<(), String> {
        let options = WriteOptions::default();
        self.submit_payload(key, payload, shown, &options).map(drop)
    }

    /// [`Self::write_payload`] as `options` say, saying what the write saw
    /// as it was queued. Control thread.
    pub(crate) fn submit_payload(
        &self,
        key: &str,
        payload: Payload,
        shown: ControlValue,
        options: &WriteOptions,
    ) -> Result<Queued, String> {
        let index = self.payload_index(key)?;
        self.deliver(index, Write::Payload(payload, shown, None), options, None)
    }

    /// [`Self::write_payload`] for a payload built against the live graph's
    /// control-surface directory: refused as [`STALE`] when a graph change
    /// published since `checked.generation`, and recorded as
    /// `checked.record` says, submitted as `options` say. Control thread.
    pub(crate) fn write_payload_checked(
        &self,
        (key, payload, shown): (&str, Payload, ControlValue),
        checked: Checked<'_>,
        options: &WriteOptions,
    ) -> Result<Queued, String> {
        let index = self.payload_index(key)?;
        let write = Write::Payload(payload, shown, Some(checked));
        self.deliver(index, write, options, None)
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

    /// Whether `key` is a payload control.
    pub(crate) fn is_payload(&self, key: &str) -> bool {
        self.payload_decl(key).is_some()
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
        let event = self.table.decl(index).is_some_and(|(decl, _)| decl.event);
        let shown = self.table.key(index).zip(self.table.value(index, raw));
        Ok(Resolved {
            index,
            raw,
            event,
            shown,
        })
    }

    /// The payload control `key` names, with its codec: `None` when it
    /// names none, or an index past the read-only count it is listed below.
    fn payload_decl(&self, key: &str) -> Option<(ControlIndex, Option<PayloadCodec>)> {
        let index = self.index(key).ok()?;
        let (decl, _) = self.table.decl(index)?;
        (decl.kind == DeclKind::Payload).then_some((index, decl.codec))
    }

    /// The payload control `key` names, refused as [`Self::index`] refuses
    /// it, or as no payload control.
    fn payload_index(&self, key: &str) -> Result<ControlIndex, String> {
        let index = self.index(key)?;
        match self.table.decl(index) {
            Some((decl, _)) if decl.kind == DeclKind::Payload => Ok(index),
            _ => Err(format!("Control '{key}' is not a payload control")),
        }
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
        let prepared = (codec.prepare)(&self.context(index), value);
        Some(prepared.map(|(payload, shown)| (index, payload, shown)))
    }

    /// What a write to `index` is built or read with: its position, and
    /// the parts its module attached.
    fn context(&self, index: ControlIndex) -> ControlContext<'_> {
        ControlContext {
            position: self.table.decl(index).map_or(0, |(_, position)| position),
            parts: &self.context,
        }
    }

    /// `value` as `index` holds it: read by its module when it declares a
    /// reader (see [`crate::control_request::ValueReader`]), else by its
    /// kind.
    fn coerce(&self, index: ControlIndex, value: &ControlValue) -> Result<RtValue, String> {
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
    /// its own (a development's alias records and legacy aliases) asks
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
    fn index(&self, key: &str) -> Result<ControlIndex, String> {
        self.table
            .resolve_present(key, |index| self.cells.load(index))
    }

    /// `authored` names the control an authored write is to, as written.
    fn deliver(
        &self,
        index: ControlIndex,
        write: Write,
        options: &WriteOptions,
        authored: Option<&str>,
    ) -> Result<Queued, String> {
        let (event, clamp, whole) = match self.table.decl(index) {
            Some((decl, _)) => (decl.event, decl.clamp.filter(|_| self.clamps), decl.whole),
            None => (false, None, false),
        };
        let route = {
            let mut route = self.route.lock().unwrap();
            if let Route::Building(written) = &mut *route {
                // Under the route lock, so a bind sees it or it sees the bind.
                let value = match write {
                    Write::Value(value) => value,
                    Write::Payload(payload, shown, checked) => {
                        self.show(index, shown, Some(payload));
                        if let Some(checked) = checked {
                            (checked.record)();
                        }
                        written.retain(|earlier| *earlier != index);
                        written.push(index);
                        return Ok(Queued::default());
                    }
                };
                // Held as the module will hold it once it applies it.
                let held = match (value, clamp) {
                    (RtValue::F32(number), _) if whole => {
                        let (min, max) = clamp.unwrap_or((f32::MIN, f32::MAX));
                        RtValue::F32(number.trunc().max(min).min(max))
                    }
                    (RtValue::F32(number), Some((min, max))) => {
                        RtValue::F32(number.max(min).min(max))
                    }
                    (RtValue::I32(whole), Some((min, max))) => {
                        RtValue::I32(whole.clamp(min as i32, max as i32))
                    }
                    _ => value,
                };
                self.cells.publish(index, held);
                written.retain(|earlier| *earlier != index);
                written.push(index);
                return Ok(Queued::default());
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
                let (value, shown, mut checked) = match write {
                    Write::Value(value) => (RequestValue::Value(value), None, None),
                    Write::Payload(payload, shown, checked) => {
                        (RequestValue::Payload(payload), Some(shown), checked)
                    }
                };
                if checked
                    .as_ref()
                    .is_some_and(|checked| checked.generation != Some(target.generation))
                {
                    return Err(STALE.into());
                }
                if let Some(checked) = &checked {
                    if publisher.timed_races(&port.module_id, &checked.targets) {
                        // Sure of its room first, so a full queue never
                        // cancels the edit, as a change publishing is.
                        if !port.requests.has_room() {
                            return Err(QUEUE_FULL.into());
                        }
                        let cause = || CancelCause::ScheduleWrite {
                            module_id: port.module_id.clone(),
                        };
                        if !publisher.cancel_timed(cause) {
                            return Err(STALE.into());
                        }
                    }
                }
                // A write retargeting its module travels in the topology
                // recompiled for it (see `publish::retarget`).
                let recompile = match &mut checked {
                    Some(checked) => {
                        let targets = std::mem::take(&mut checked.targets);
                        port.retarget.prepare(&publisher, &port.module_id, targets)
                    }
                    None => None,
                };
                let value = match (recompile, value) {
                    // A schedule that changes its targets travels in the
                    // recompile's publication, not as a request: it lands
                    // whatever its rank or ttl, as the last write made, and
                    // reads back as what plays.
                    (Some(prepared), RequestValue::Payload(payload)) => {
                        let change = (prepared, port.module_id.as_str());
                        let handed = (index, payload);
                        let published =
                            port.retarget
                                .publish(&mut publisher, &port.pending, change, handed);
                        let published = match published {
                            Ok(published) => published,
                            Err((error, refused)) => {
                                drop(publisher);
                                drop(refused);
                                return Err(error);
                            }
                        };
                        if let Some(shown) = shown {
                            self.show(index, shown, None);
                        }
                        if let Some(checked) = checked {
                            (checked.record)();
                        }
                        // What the recompile replaced is freed off the lock.
                        drop(publisher);
                        drop(published);
                        if let Some(settler) = &port.settler {
                            settler.settle();
                        }
                        // No timed edit waits (`Retarget::prepare` publishes
                        // none while one does), so none can overtake it, and
                        // a payload is never kept.
                        return Ok(Queued {
                            safe: None,
                            id: None,
                            lost: false,
                        });
                    }
                    (_, value) => value,
                };
                // A payload is listed pending by its read-back and counted
                // unlogged once applied: the session log cannot carry it yet
                // (see `PendingWrite::raw`).
                let (read, raw) = match (&value, &shown) {
                    (RequestValue::Value(value), _) => {
                        (self.table.value(index, *value), Some(*value))
                    }
                    (_, shown) => (shown.clone(), None),
                };
                let mut request = Request::new(target, value);
                request.event = event;
                request.intent = match options.authored {
                    Some(_) => Intent::Author,
                    None => Intent::Perform,
                };
                request.source = options.source;
                request.priority = options.priority;
                request.ttl = options.ttl;
                let automated = request.source == Source::Automation;
                // Never kept on a graph with no clock: there the copy's
                // hold, at a sample that never passes, would refuse every
                // later write to the control. The edit is cancelled instead.
                let clocked = port.settler.is_none();
                // The log is locked before anything is submitted, the copy
                // included, so neither's outcome can be received before
                // the write is recorded.
                let mut pending = port.pending.lock().unwrap();
                pending.settle();
                let kept = match (&request.value, authored) {
                    (RequestValue::Value(raw), Some(key)) if clocked => {
                        let module = (port.module_id.as_str(), key);
                        let write = (*raw, event, options);
                        publisher.keep_authored(module, target, write, &port.requests)
                    }
                    // Any other authored write (a payload, one on a graph
                    // with no clock) queued after an edit landed follows its
                    // writes too, as one kept would.
                    _ if options.authored.is_some() => {
                        publisher.timed_landed().map(|id| (id, None))
                    }
                    _ => None,
                };
                let (safe, copy) = (kept.map(|(safe, _)| safe), kept.and_then(|(_, copy)| copy));
                let id = match port.requests.submit(request) {
                    Ok(id) => id,
                    // Never, as `keep_authored` kept it only with room for
                    // this write too, but a write kept and not made is not
                    // to play: the edit is cancelled, the copy with it.
                    // Landed already, the copy plays: it is the write.
                    Err(_) => match (copy, authored) {
                        (Some(copy), Some(key)) => {
                            let cause = || CancelCause::AuthoredWrite {
                                module_id: port.module_id.clone(),
                                key: key.to_string(),
                                touches: Touched::Written,
                            };
                            if publisher.cancel_timed(cause) {
                                return Err(QUEUE_FULL.into());
                            }
                            copy
                        }
                        _ => return Err(QUEUE_FULL.into()),
                    },
                };
                // Under the publisher's lock, so reads report the last write
                // submitted.
                if let Some(shown) = shown {
                    self.show(index, shown, None);
                }
                if let (Some(key), Some(written)) = (self.table.key(index), read) {
                    let module_id = port.module_id.clone();
                    let write = PendingWrite {
                        module_id,
                        key,
                        value: written,
                        raw,
                        beat: None,
                        automated,
                        announced: options.announced,
                        // Safe from the timed edit it was kept over, or that
                        // landed before it: its record follows that edit's.
                        authored: options.authored.map(|authoring| Authoring {
                            safe,
                            ..authoring.clone()
                        }),
                    };
                    // The copy applying, too, is this write's, though only
                    // the write is announced.
                    if let Some(copy) = copy.filter(|copy| *copy != id) {
                        let copy_write = PendingWrite {
                            announced: false,
                            ..write.clone()
                        };
                        pending.submitted(copy, copy_write);
                    }
                    pending.submitted(id, write);
                }
                drop(pending);
                if let Some(checked) = checked {
                    (checked.record)();
                }
                // Settled with no lock held: other writers carry on, and a
                // request one of them queues meanwhile applies in the same
                // block or theirs.
                drop(publisher);
                if let Some(settler) = &port.settler {
                    settler.settle();
                }
                Ok(Queued {
                    safe,
                    id: Some(id),
                    lost: false,
                })
            }
            Route::Offline { graph, module_id } => {
                let rank = Rank::new(options.source, options.priority);
                let graph = graph.upgrade().ok_or("The render has been replaced")?;
                let mut graph = graph.lock().unwrap();
                self.check_still(|route| matches!(route, Route::Offline { .. }))?;
                match write {
                    Write::Value(value) => graph.apply_control(&module_id, index, value, rank),
                    Write::Payload(payload, shown, checked) => {
                        // Resolved before an offline edit installed: it may
                        // name what the edit replaced (see `Checked`).
                        let generation = checked.as_ref().and_then(|c| c.generation);
                        if generation.is_some_and(|seen| seen != graph.offline_edits) {
                            return Err(STALE.into());
                        }
                        let applied = graph.apply_payload(&module_id, index, payload, rank);
                        if applied == Ok(true) {
                            self.show(index, shown, None);
                            if let Some(checked) = checked {
                                (checked.record)();
                                graph.recompile_if_retargeted(&module_id);
                            }
                        }
                        applied
                    }
                }
                .map(|applied| Queued {
                    lost: !applied,
                    ..Queued::default()
                })
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
        self.submit(key, value, &WriteOptions::default()).map(drop)
    }

    #[allow(private_interfaces)]
    fn submit(
        &self,
        key: &str,
        value: ControlValue,
        options: &WriteOptions,
    ) -> Result<Queued, String> {
        self.set_control_queued(key, value, options)
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        let decl = (self.index(key).ok()).and_then(|index| self.table.decl(index));
        if decl.is_some_and(|(decl, _)| decl.loads_on_write) {
            return value.as_string().map(drop);
        }
        if let Some(prepared) = self.prepare(key, value) {
            return prepared.map(drop);
        }
        let index = self.index(key)?;
        self.coerce(index, value).map(drop)
    }

    /// A payload built with this surface's context, which a development
    /// aliasing it does not hold: `None` when it holds none, and the
    /// development's own surface builds the write from the value alone.
    #[allow(private_interfaces)]
    fn prepare_payload(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Option<Result<(Payload, ControlValue), String>> {
        if self.context.is_empty() {
            return None;
        }
        let prepared = self.prepare(key, value)?;
        Some(prepared.map(|(_, payload, shown)| (payload, shown)))
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
            // In the order written, each scalar as written: read before any
            // payload applies, as a payload republishes the cells it
            // rewrites. What a payload replaces, or a refused one, is
            // dropped here, on this control thread.
            let values: Vec<_> = written
                .iter()
                .map(|index| self.cells.load(*index))
                .collect();
            let mut payloads = self.payloads.lock().unwrap();
            for (index, value) in written.iter().zip(values) {
                match payloads.iter_mut().find(|payload| payload.index == *index) {
                    Some(payload) => {
                        if let Some(staged) = payload.staged.take() {
                            let applied = module.apply_payload(*index, staged);
                            debug_assert!(applied.is_ok(), "a module takes what its codec builds");
                        }
                    }
                    None => {
                        if let Some(value) = value {
                            let _ = apply_declared(module, *index, value);
                        }
                    }
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
        self.index(key).is_ok()
    }

    fn set_legacy(&self, key: &str, _value: ControlValue) -> Result<(), String> {
        self.index(key).map(drop)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn declaration(&self, key: &str) -> Option<Declaration> {
        let index = self.index(key).ok()?;
        Some(Declaration {
            index,
            decl: self.table.decl(index)?.0.clone(),
            current: self.cells.load(index)?,
        })
    }

    fn automation(&self, key: &str) -> Option<Automation> {
        let index = self.index(key).ok()?;
        let (decl, _) = self.table.decl(index)?;
        // An event is automated too: a schedule entry on it fires it, at
        // most once a block (automation writes coalesce per block).
        // A control its module reads (a kit's `play`, naming a slot) is not:
        // a schedule writes the raw value, not what the module reads.
        let writable = decl.writer == Writer::Parameter && decl.reader.is_none();
        let scalar = matches!(
            decl.kind,
            DeclKind::Number { .. } | DeclKind::Integer { .. } | DeclKind::Bool
        );
        (writable && scalar).then(|| Automation {
            cells: self.cells.clone(),
            index,
            kind: decl.kind,
            clamp: decl.clamp,
            event: decl.event,
            whole: decl.whole,
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
            surface.bind(offline_route(graph, id), instance.module_mut());
        }
    }
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
