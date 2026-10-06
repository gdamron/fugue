use crate::invention::graph::{GraphCommand, SignalGraph};
use crate::invention::publish::{edge, BuiltModule, GraphChange, LiveGraph};
use crate::invention::runtime::{ControlSurfaceInstance, GraphCommandError};
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
    /// when the write is meant as the module's new starting state (FUG-266).
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

/// Cloneable read-oriented view over runtime state and control surfaces.
#[derive(Clone)]
pub struct RuntimeSnapshot {
    pub state: Arc<Mutex<RuntimeState>>,
    pub control_surfaces: Arc<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
    /// Where recorded control writes announce themselves; empty until a host
    /// installs a sink. Offline render runtimes leave it empty.
    pub(crate) event_sink: EventSinkSlot,
}

/// Cloneable mutation handle used by orchestration hosts and external APIs.
///
/// Live runtimes publish structural changes through the runtime's single
/// publisher (see [`crate::invention::publish`]), while render runtimes apply
/// the same edits directly to the in-memory graph.
#[derive(Clone)]
pub struct RuntimeController {
    pub(crate) snapshot: RuntimeSnapshot,
    pub(crate) registry: ModuleRegistry,
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
    /// *applied* value.
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
    /// of them as a `ControlChanged` event regardless of intent (finding
    /// FUG-239 #7). What intent changes is whether the value is *authored*:
    /// a [`ControlWriteIntent::Perform`] write is a live gesture that must
    /// neither land in a saved document nor advance the daemon's revision, so
    /// a scheduler running at musical rate cannot make every peer's structural
    /// edit stale (FUG-266).
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
        let applied = if intent.is_authoring() {
            self.set_control_recorded(module_id, key, value)?
        } else {
            self.set_control_performed(module_id, key, value)?
        };
        self.emit_control_changed(module_id, key, applied);
        Ok(())
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
    ) -> Result<ControlValue, GraphCommandError> {
        let value = self.coerced(module_id, key, value)?;
        self.set_control_transient(module_id, key, value.clone())?;
        Ok(value)
    }

    /// Coerces, applies, and records a control write without emitting an event,
    /// returning the applied (coerced) value: the authored half of
    /// [`Self::set_control_with_intent`], which emits the event. Records into
    /// the retained document and the module's stored config (see
    /// `RuntimeState::record_authored_control`).
    pub(crate) fn set_control_recorded(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<ControlValue, GraphCommandError> {
        // Coerce to the control's declared kind before applying and recording,
        // so a stringified write lands and the retained document stays typed
        // (see FUG-240). set_control_transient stays uncoerced: its callers are
        // internal telemetry writes that already carry the right type.
        let value = self.coerced(module_id, key, value)?;
        self.set_control_transient(module_id, key, value.clone())?;
        self.state
            .lock()
            .unwrap()
            .record_authored_control(module_id, key, &value);
        Ok(value)
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

    /// Announces a recorded control change to the installed event sink, if any.
    pub(crate) fn emit_control_changed(&self, module_id: &str, key: &str, value: ControlValue) {
        // Clone the sink out under the lock, then release it before emitting so
        // a sink implementation can never re-enter this slot while we hold it.
        let sink = self.event_sink.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink.emit(RpcEvent::new(RpcEventPayload::ControlChanged {
                module_id: module_id.to_string(),
                key: key.to_string(),
                value,
            }));
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
        // Invoke outside the directory lock: a scheduler's `schedule` write
        // re-resolves its targets against this same directory.
        let surface = {
            let controls = self.control_surfaces.lock().unwrap();
            controls
                .get(module_id)
                .cloned()
                .ok_or_else(|| GraphCommandError::UnknownModule(module_id.to_string()))?
        };
        surface
            .set_control(key, value)
            .map_err(GraphCommandError::ControlError)
    }

    /// Returns the declarative document describing the current graph (see
    /// [`RuntimeState::document`]).
    pub fn document(&self) -> Option<crate::Invention> {
        self.state.lock().unwrap().document()
    }
}

impl RuntimeController {
    /// Applies an edit directly to an offline render's graph.
    fn apply(&self, cmd: GraphCommand) -> Result<(), GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or(GraphCommandError::AudioThreadStopped)?;
        graph.lock().unwrap().apply_command(cmd);
        Ok(())
    }

    /// Builds and inserts a module into the current graph.
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
        let BuiltModule {
            instance,
            info,
            module,
            surface,
            handles,
        } = GraphChange::build(
            &self.registry,
            self.sample_rate,
            module_id,
            module_type,
            config,
        )?;

        // Attach schedulers before touching the graph, so a schedule that
        // fails to resolve leaves the running invention unchanged.
        if module_type == crate::modules::control_scheduler::CONTROL_SCHEDULER_TYPE_ID {
            let handle = handles
                .iter()
                .find(|(name, _)| name == "controls")
                .map(|(_, handle)| handle);
            crate::modules::control_scheduler::attach_from_handle(
                module_id,
                handle,
                &self.snapshot.control_surfaces,
            )
            .map_err(GraphCommandError::ModuleBuildFailed)?;
        }

        if let Some(control_surface) = surface {
            self.snapshot
                .control_surfaces
                .lock()
                .unwrap()
                .insert(module_id.to_string(), control_surface);
        }

        if let Some(module) = instance {
            self.apply(GraphCommand::AddModule {
                module_id: module_id.to_string(),
                module,
            })?;
        }

        self.module_ports
            .lock()
            .unwrap()
            .insert(module_id.to_string(), module.ports);

        {
            let mut state = self.snapshot.state.lock().unwrap();
            state.modules.insert(module_id.to_string(), info);
            state.document_upsert_module(module_id, module_type, config);
        }

        Ok(handles
            .into_iter()
            .map(|(name, handle)| (format!("{}.{}", module_id, name), handle))
            .collect())
    }

    /// Removes a module and any connections that reference it.
    pub fn remove_module(&self, module_id: &str) -> Result<(), GraphCommandError> {
        if let Some(live) = &self.live {
            return live.remove_module(module_id).map(drop);
        }
        self.snapshot
            .control_surfaces
            .lock()
            .unwrap()
            .shift_remove(module_id);
        self.apply(GraphCommand::RemoveModule {
            module_id: module_id.to_string(),
        })?;
        self.module_ports.lock().unwrap().shift_remove(module_id);
        let mut state = self.snapshot.state.lock().unwrap();
        state.modules.shift_remove(module_id);
        state
            .connections
            .retain(|conn| conn.from != module_id && conn.to != module_id);
        state.document_remove_module(module_id);
        Ok(())
    }

    /// Connects an output port to an input port after validating both ends.
    pub fn connect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        if let Some(live) = &self.live {
            return live.connect(edge(from_module, from_port, to_module, to_port));
        }
        let ports = self.module_ports.lock().unwrap();
        let source = ports
            .get(from_module)
            .ok_or_else(|| GraphCommandError::UnknownModule(from_module.to_string()))?;
        if !source.outputs.iter().any(|port| port == from_port) {
            return Err(GraphCommandError::InvalidPort(format!(
                "module '{}' does not have output port '{}' (available: {:?})",
                from_module, from_port, source.outputs
            )));
        }
        let dest = ports
            .get(to_module)
            .ok_or_else(|| GraphCommandError::UnknownModule(to_module.to_string()))?;
        if !dest.inputs.iter().any(|port| port == to_port) {
            return Err(GraphCommandError::InvalidPort(format!(
                "module '{}' does not have input port '{}' (available: {:?})",
                to_module, to_port, dest.inputs
            )));
        }
        drop(ports);

        self.apply(GraphCommand::AddConnection {
            from_module: from_module.to_string(),
            from_port: from_port.to_string(),
            to_module: to_module.to_string(),
            to_port: to_port.to_string(),
        })?;

        self.snapshot
            .state
            .lock()
            .unwrap()
            .connections
            .push(RuntimeConnectionInfo {
                from: from_module.to_string(),
                from_port: from_port.to_string(),
                to: to_module.to_string(),
                to_port: to_port.to_string(),
            });
        Ok(())
    }

    /// Removes a connection between two ports if present.
    pub fn disconnect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        if let Some(live) = &self.live {
            return live.disconnect(edge(from_module, from_port, to_module, to_port));
        }
        self.apply(GraphCommand::RemoveConnection {
            from_module: from_module.to_string(),
            from_port: from_port.to_string(),
            to_module: to_module.to_string(),
            to_port: to_port.to_string(),
        })?;
        self.snapshot
            .state
            .lock()
            .unwrap()
            .connections
            .retain(|conn| {
                !(conn.from == from_module
                    && conn.from_port == from_port
                    && conn.to == to_module
                    && conn.to_port == to_port)
            });
        Ok(())
    }
}
