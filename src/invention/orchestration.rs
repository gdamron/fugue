use crate::control_request::{RequestId, Source};
use crate::invention::declared::{Queued, WriteOptions};
use crate::invention::graph::SignalGraph;
use crate::invention::publish::{edge, edit_offline, Authoring, Committed, GraphChange, LiveGraph};
use crate::invention::runtime::{control_timing, ControlSurfaceInstance, GraphCommandError};
use crate::registry::ModuleRegistry;
use crate::rpc::non_finite_refusal;
use crate::{
    ControlMeta, ControlValue, ControlWrite, ControlWriteIntent, RpcEvent, RpcEventPayload,
    RpcEventSink,
};
use indexmap::IndexMap;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::state::{RuntimeConnectionInfo, RuntimeModuleInfo, RuntimeState, RuntimeStatus};

pub(crate) mod authored;

/// Read/write orchestration surface shared by live and render runtimes.
pub trait OrchestrationRuntime {
    /// Returns the current runtime status.
    fn status(&self) -> RuntimeStatus;
    /// Returns the current module snapshot.
    fn list_modules(&self) -> Vec<RuntimeModuleInfo>;
    /// Returns the current connection snapshot.
    fn list_connections(&self) -> Vec<RuntimeConnectionInfo>;
    /// Returns control metadata for one module or for all modules with controls.
    fn list_controls(
        &self,
        module_id: Option<&str>,
    ) -> Result<Vec<(String, Vec<ControlMeta>)>, GraphCommandError>;
    /// Reads a control value from a specific module.
    fn get_control(&self, module_id: &str, key: &str) -> Result<ControlValue, GraphCommandError>;
    /// Updates a control value on a specific module.
    fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), GraphCommandError>;

    /// Updates a control value, recording it in the retained document only
    /// when the write is meant as the module's new starting state.
    ///
    /// Defaults to [`Self::set_control`] — authoring — for runtimes with no
    /// concurrent clients to conflict with, notably offline render.
    fn set_control_with_intent(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
        intent: ControlWriteIntent,
    ) -> Result<(), GraphCommandError> {
        let _ = intent;
        self.set_control(module_id, key, value)
    }

    /// Applies a batch of control writes in order within one call, so a
    /// multi-control conducting gesture lands together rather than smeared
    /// across separate requests. Each write carries its own intent and goes
    /// through [`Self::set_control_with_intent`] (same coercion, same refusal
    /// of non-finite numbers). Fails on the first bad write;
    /// writes already applied before it stand, since control writes have no
    /// rollback — validate keys with `list_controls` first if that matters.
    fn set_controls(&self, writes: &[ControlWrite]) -> Result<(), GraphCommandError> {
        for write in writes {
            self.set_control_with_intent(
                &write.module_id,
                &write.key,
                write.value.clone(),
                write.intent,
            )?;
        }
        Ok(())
    }
}

/// A late-bindable event sink shared by every clone of a runtime's snapshot.
///
/// Built empty; the daemon installs a sink after `start()` (see
/// [`crate::RunningInvention::set_event_sink`]). Scripts and agents capture a
/// snapshot at build time — before the sink lands — so the slot is shared by
/// `Arc` and read at emit time, letting those already-running writers observe
/// the sink once it is installed. Emission runs on control/script threads, never
/// the audio callback, so the mutex here does not touch the hot path.
pub type EventSinkSlot = Arc<Mutex<Option<Arc<dyn RpcEventSink>>>>;

/// What a front door is told when a control write is taken: the value as
/// coerced, and the request it was submitted as. `request` is `None` when
/// the write applied as it was made (offline, while building, an
/// undeclared control) or travels in `published`, the publication it applies with;
/// `lost` when it lost to a higher-ranked write as it was made and
/// applied nothing.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ControlAck {
    pub(crate) value: ControlValue,
    pub(crate) request: Option<RequestId>,
    pub(crate) lost: bool,
    pub(crate) published: Option<u64>,
}

impl ControlAck {
    fn new(value: ControlValue, queued: &Queued) -> Self {
        Self {
            value,
            request: queued.id,
            lost: queued.lost,
            published: queued.published,
        }
    }
}

/// Cloneable read-oriented view over runtime state and control surfaces.
#[derive(Clone)]
pub struct RuntimeSnapshot {
    pub state: Arc<Mutex<RuntimeState>>,
    pub control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
    /// Where recorded control writes announce themselves; empty until a host
    /// installs a sink. Offline render runtimes leave it empty.
    pub(crate) event_sink: EventSinkSlot,
    /// The live graph, whose request log counts the writes that reach a
    /// module without a request. `None` offline.
    pub(crate) live: Option<LiveGraph>,
}

/// Cloneable mutation handle used by orchestration hosts and external APIs.
///
/// Live runtimes publish structural changes through the runtime's single
/// publisher (see [`crate::invention::publish`]), while render runtimes apply
/// the same edits directly to the in-memory graph.
#[derive(Clone)]
pub struct RuntimeController {
    pub(crate) snapshot: RuntimeSnapshot,
    /// The registry an offline render builds modules against: `Some` exactly
    /// when `graph` is. A live controller has none of its own; it builds
    /// through `live`, against the live graph's current registry.
    pub(crate) registry: Option<ModuleRegistry>,
    pub(crate) sample_rate: u32,
    pub(crate) graph: Option<Arc<Mutex<SignalGraph>>>,
    pub(crate) live: Option<LiveGraph>,
    pub(crate) module_ports: Arc<Mutex<IndexMap<String, ModulePorts>>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ModulePorts {
    pub(crate) inputs: Vec<String>,
    pub(crate) outputs: Vec<String>,
}

impl RuntimeSnapshot {
    /// Returns aggregate status for the current invention.
    pub fn status(&self) -> RuntimeStatus {
        self.state.lock().unwrap().status()
    }

    /// Returns a copy of the current module snapshot.
    pub fn list_modules(&self) -> Vec<RuntimeModuleInfo> {
        self.state
            .lock()
            .unwrap()
            .modules
            .values()
            .cloned()
            .collect()
    }

    /// Returns a copy of the current connection snapshot.
    pub fn list_connections(&self) -> Vec<RuntimeConnectionInfo> {
        self.state.lock().unwrap().connections.clone()
    }

    /// Lists controls for a single module or all modules with control surfaces.
    pub fn list_controls(
        &self,
        module_id: Option<&str>,
    ) -> Result<Vec<(String, Vec<ControlMeta>)>, GraphCommandError> {
        let controls = self.control_surfaces.lock().unwrap();
        if let Some(module_id) = module_id {
            let surface = controls
                .get(module_id)
                .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
            return Ok(vec![(module_id.to_string(), surface.controls())]);
        }

        let mut result = Vec::new();
        for (id, surface) in controls.iter() {
            let metadata = surface.controls();
            if !metadata.is_empty() {
                result.push((id.clone(), metadata));
            }
        }
        Ok(result)
    }

    /// Reads the current value of a module control.
    pub fn get_control(
        &self,
        module_id: &str,
        key: &str,
    ) -> Result<ControlValue, GraphCommandError> {
        let controls = self.control_surfaces.lock().unwrap();
        let surface = controls
            .get(module_id)
            .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?;
        surface
            .get_control(key)
            .map_err(GraphCommandError::ControlError)
    }

    /// Sets the current value of a module control as an *authoring* change:
    /// records it in the retained document so it survives a save/rebuild, and
    /// announces it as a [`RpcEventPayload::ControlChanged`] carrying the
    /// *applied* value once it applies.
    ///
    /// Equivalent to [`Self::set_control_with_intent`] with
    /// [`ControlWriteIntent::Author`].
    pub fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), GraphCommandError> {
        self.set_control_with_intent(module_id, key, value, ControlWriteIntent::Author)
    }

    /// Sets a module control, recording it in the retained document only when
    /// the write is meant as the module's new starting state.
    ///
    /// This is the choke point for externally-initiated control writes — RPC
    /// commands, conducting scripts, and agents — so an observer sees every one
    /// of them that applies as a `ControlChanged` event regardless of intent.
    /// A write applied as it was made is announced now; one submitted as a
    /// request once it applies, naming the request, the next time this or
    /// [`Self::announce_applied`] runs. One that never applies (superseded,
    /// overridden, refused) is never announced. What intent changes is
    /// whether the value is *authored*: a [`ControlWriteIntent::Perform`]
    /// write is a live gesture that must neither land in a saved document
    /// nor advance the daemon's revision, so a scheduler running at musical
    /// rate cannot make every peer's structural edit stale.
    ///
    /// Every write is coerced to the control's declared kind first. A write
    /// whose value is a number that is not finite after coercion (NaN, an
    /// infinity, or a value too large for an `f32`; for a number control this
    /// includes the strings `"NaN"`, `"inf"` and `"1e39"`) is refused as
    /// [`GraphCommandError::ControlError`] before it reaches the module or the
    /// document: neither changes and no event is emitted. A string control
    /// coerces such a number to text (`"NaN"`) and accepts it. A document
    /// cannot hold a non-finite number (JSON would record `null`), and DSP
    /// code fed one goes silent or blasts noise.
    ///
    /// Internal writes that must stay silent go around this: a reload or an
    /// `ApplyEdits` commit writes surviving modules with
    /// [`Self::set_control_transient`] (its retained document already holds
    /// the values) and records a failed write's actual value with
    /// `RuntimeState::record_authored_control`.
    pub fn set_control_with_intent(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
        intent: ControlWriteIntent,
    ) -> Result<(), GraphCommandError> {
        let options = WriteOptions::performed(Source::User);
        self.submit_control(module_id, key, value, intent, options)
            .map(drop)
    }

    /// Submits a control write as [`Self::set_control_with_intent`] does,
    /// for a front door that says who it is and how the write ranks
    /// (`options`; whether it is authored comes from `intent`), and tells it
    /// what was taken: the value as coerced and the request it was submitted
    /// as, when it was one.
    pub(crate) fn submit_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
        intent: ControlWriteIntent,
        options: WriteOptions,
    ) -> Result<ControlAck, GraphCommandError> {
        let options = options.performing().announcing();
        // An event (a clock's reset) fires; there is no state to author.
        let ack = if intent.is_authoring() && !self.is_event(module_id, key) {
            self.set_control_recorded(module_id, key, value, &options)?
        } else {
            self.set_control_performed(module_id, key, value, &options)?
        };
        // Those that applied earlier go first.
        self.announce_applied();
        // One submitted as a request is announced once it applies, maybe
        // already. One travelling in a publication applies with it, at the
        // next block unless the stream stalls, and is announced now.
        if ack.request.is_none() && !ack.lost {
            self.emit_control_changed(module_id, key, ack.value.clone(), None);
        }
        Ok(ack)
    }

    /// Announces each front door's write that applied since the last call
    /// as a `ControlChanged` event naming its request (see
    /// [`Self::set_control_with_intent`]), and each timed one that settled,
    /// however it did, as a `ControlRequestSettled` event after it. A host
    /// calls it now and then, so a write that applied after its reply is
    /// announced. Offline, every write was announced as it was made.
    pub fn announce_applied(&self) {
        let Some(live) = &self.live else {
            return;
        };
        for applied in live.take_announcements() {
            let settled = control_timing::settled(&applied);
            if applied.applied() {
                let (module_id, key) = (&applied.module_id, &applied.key);
                let request = Some(applied.request.0.to_string());
                self.emit_control_changed(module_id, key, applied.value, request);
            }
            if let Some(settled) = settled {
                self.emit(RpcEventPayload::ControlRequestSettled(settled));
            }
        }
    }

    /// Coerces and applies a control write without recording it in the
    /// retained document, returning the applied (coerced) value.
    ///
    /// The performance counterpart to [`Self::set_control_recorded`]: same
    /// coercion, same live effect, but the authored document is untouched.
    /// Distinct from [`Self::set_control_transient`], which additionally skips
    /// coercion because its callers are internal telemetry writes that already
    /// carry the right type.
    fn set_control_performed(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
        options: &WriteOptions,
    ) -> Result<ControlAck, GraphCommandError> {
        let value = self.coerced(module_id, key, value)?;
        let (load, _) = self.asset_to_load(key, &value)?;
        let queued = self.submit_transient(module_id, key, load, options)?;
        Ok(ControlAck::new(value, &queued))
    }

    /// Coerces and submits an authored control write without emitting an
    /// event, returning the coerced value and the request it was submitted
    /// as: the authored half of [`Self::set_control_with_intent`], which
    /// emits the event. The write is recorded into the retained document and
    /// the module's stored config (see `RuntimeState::record_authored_control`)
    /// once it applies, which may be after this returns.
    pub(crate) fn set_control_recorded(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
        options: &WriteOptions,
    ) -> Result<ControlAck, GraphCommandError> {
        // Coerce to the control's declared kind before applying and recording,
        // so a stringified write lands and the retained document stays typed.
        // set_control_transient stays uncoerced: its callers are
        // internal telemetry writes that already carry the right type.
        let value = self.coerced(module_id, key, value)?;
        let (load, origin) = self.asset_to_load(key, &value)?;
        let Some(live) = &self.live else {
            let queued = self.submit_transient(module_id, key, load, options)?;
            // Offline, a write applies as it is made, unless it lost.
            if !queued.lost {
                let mut state = self.state.lock().unwrap();
                state.record_authored_control(module_id, key, &value);
            }
            return Ok(ControlAck::new(value, &queued));
        };
        // Submitted as a request, the write is recorded in the document
        // once it applies, in the order the audio thread applied it (see
        // `publish::authored`): one refused, lost to a higher rank or
        // replaced at its sample is never recorded. One applied as it was
        // made (an undeclared control, a schedule travelling in a publication)
        // is recorded now. A timed edit waiting that rebuilds or removes
        // the module is cancelled. Authored writers take turns, so the
        // last submitted is the last recorded. A timed edit already landed
        // is adopted first, so the write goes to the module playing.
        let _authoring = live.authoring();
        live.adopt_landed();
        let authoring = Authoring {
            value,
            origin,
            safe: None,
            then: Vec::new(),
            timed: options.time.is_some(),
            chained: false,
        };
        let queued = self.make_authored(module_id, key, load, &options.authored_as(&authoring))?;
        // A timed write is ordered against a timed edit by when each
        // applies, not reconciled with it (see `publish::timed_write`).
        if !authoring.timed {
            let now = queued.id.is_none().then_some(&authoring);
            live.reconcile_authored((module_id, key), queued.safe, now);
        }
        Ok(ControlAck::new(authoring.value, &queued))
    }

    /// Whether `key` of `module_id` is a declared event control.
    pub(crate) fn is_event(&self, module_id: &str, key: &str) -> bool {
        let surfaces = self.control_surfaces.lock().unwrap();
        surfaces
            .get(module_id)
            .and_then(|surface| surface.declaration(key))
            .is_some_and(|declaration| declaration.decl.event)
    }

    /// Coerces a value to the control's declared kind, leaving it untouched
    /// when the module is unknown (the write itself then reports the error).
    ///
    /// Refuses a number that is not finite once coerced, with the same message
    /// the edit path gives, so no live write can hand one to a module setter
    /// or record it in the document.
    fn coerced(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<ControlValue, GraphCommandError> {
        let value = {
            let controls = self.control_surfaces.lock().unwrap();
            match controls.get(module_id) {
                Some(surface) => surface.coerce_value(key, value),
                None => return Ok(value),
            }
        };
        match non_finite_refusal(module_id, key, &value) {
            Some(message) => Err(GraphCommandError::ControlError(message)),
            None => Ok(value),
        }
    }

    /// Announces an applied control change to the installed event sink, if
    /// any, naming the request it applied as.
    pub(crate) fn emit_control_changed(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
        request: Option<String>,
    ) {
        self.emit(RpcEventPayload::ControlChanged {
            module_id: module_id.to_string(),
            key: key.to_string(),
            value,
            request,
        });
    }

    /// Emits `payload` to the installed event sink, if any.
    fn emit(&self, payload: RpcEventPayload) {
        // Clone the sink out under the lock, then release it before emitting so
        // a sink implementation can never re-enter this slot while we hold it.
        let sink = self.event_sink.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink.emit(RpcEvent::new(payload));
        }
    }

    /// Sets a control without recording it in the retained document. For
    /// runtime telemetry controls (`status`, `last_error`, agent history)
    /// that describe live activity rather than authored configuration —
    /// they must not end up in a saved invention file.
    pub fn set_control_transient(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), GraphCommandError> {
        self.submit_transient(module_id, key, value, &WriteOptions::default())
            .map(drop)
    }

    /// Makes a write as [`Self::set_control_transient`] does, submitted
    /// with `options`, saying what it saw as it was queued (see
    /// `declared::Queued`).
    pub(crate) fn submit_transient(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
        options: &WriteOptions,
    ) -> Result<Queued, GraphCommandError> {
        // Invoke outside the directory lock: a scheduler's `schedule` write
        // re-resolves its targets against this same directory.
        let surface = {
            let controls = self.control_surfaces.lock().unwrap();
            controls
                .get(module_id)
                .cloned()
                .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?
        };
        // Undeclared host state is written at once (whichever surface
        // writes it), so it cannot wait for a time.
        if options.time.is_some() && surface.writes_undeclared(key) {
            return Err(GraphCommandError::ControlError(format!(
                "Control '{key}' of '{module_id}' cannot be timed: it reaches undeclared host state, written at once"
            )));
        }
        let queued = surface
            .submit(key, value, options)
            .map_err(GraphCommandError::ControlError)?;
        // A declared control's write is a request, and logged; any other
        // reaches its module directly, as does one to a development's
        // undeclared aliases beside its declared part.
        let unlogged = surface.writes_undeclared(key) && !queued.lost;
        if let Some(live) = self.live.as_ref().filter(|_| unlogged) {
            live.note_unlogged();
        }
        Ok(queued)
    }

    /// Returns the declarative document describing the current graph (see
    /// [`RuntimeState::document`]).
    pub fn document(&self) -> Option<crate::Invention> {
        // A timed edit that has landed is adopted first, as
        // `RunningInvention::document` does, so the document is what plays.
        if let Some(live) = &self.live {
            live.adopt_landed();
        }
        self.state.lock().unwrap().document()
    }
}

impl RuntimeController {
    /// Applies `edit` to an offline render's graph as one change, as a live
    /// edit would (see [`edit_offline`]).
    fn edit_offline(
        &self,
        edit: impl FnOnce(&mut GraphChange) -> Result<(), GraphCommandError>,
    ) -> Result<Committed, GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or(GraphCommandError::AudioThreadStopped)?;
        let mirrors = (
            &self.snapshot.control_surfaces,
            &*self.module_ports,
            &*self.snapshot.state,
        );
        edit_offline(graph, mirrors, false, edit)
    }

    /// Builds and inserts a module into the current graph, replacing one
    /// with the same id in place; connections to ports the new module lacks
    /// are dropped.
    ///
    /// Returned handles are flattened as `<module_id>.<handle_name>` to match
    /// the runtime's existing handle naming scheme.
    pub fn add_module(
        &self,
        module_id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<HashMap<String, Arc<dyn Any + Send + Sync>>, GraphCommandError> {
        if let Some(live) = &self.live {
            // Built against the live graph's current registry, not this
            // controller's (see `RunningInvention::controller`).
            return live
                .add_module(self.sample_rate, module_id, module_type, config)
                .map(|committed| committed.handles);
        }
        let built = GraphChange::build(
            self.registry
                .as_ref()
                .expect("a render controller carries its registry"),
            self.sample_rate,
            module_id,
            module_type,
            config,
        )?;
        self.edit_offline(|change| {
            change.upsert(module_id, built);
            Ok(())
        })
        .map(|committed| committed.handles)
    }

    /// Removes a module and any connections that reference it. Refused,
    /// with nothing changed, while a schedule still targets it.
    pub fn remove_module(&self, module_id: &str) -> Result<(), GraphCommandError> {
        if let Some(live) = &self.live {
            return live.remove_module(module_id).map(drop);
        }
        self.edit_offline(|change| {
            change.remove(module_id);
            Ok(())
        })
        .map(drop)
    }

    /// Connects an output port to an input port after validating both ends.
    pub fn connect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        let edge = edge(from_module, from_port, to_module, to_port);
        if let Some(live) = &self.live {
            return live.connect(edge);
        }
        self.edit_offline(|change| change.connect(edge)).map(drop)
    }

    /// Removes a connection between two ports if present.
    pub fn disconnect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        let edge = edge(from_module, from_port, to_module, to_port);
        if let Some(live) = &self.live {
            return live.disconnect(edge);
        }
        self.edit_offline(|change| {
            change.disconnect(edge);
            Ok(())
        })
        .map(drop)
    }
}
