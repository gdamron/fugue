use crate::factory::ModuleBuildResult;
use crate::invention::graph::{GraphCommand, SignalGraph};
use crate::invention::runtime::{ControlSurfaceInstance, GraphCommandError};
use crate::registry::ModuleRegistry;
use crate::{ControlMeta, ControlValue, ControlWrite, RpcEvent, RpcEventPayload, RpcEventSink};
use indexmap::IndexMap;
use std::any::Any;
use std::collections::HashMap;
use std::sync::mpsc;
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

    /// Applies a batch of control writes in order within one call, so a
    /// multi-control conducting gesture lands together rather than smeared
    /// across separate requests. Each write goes through [`Self::set_control`]
    /// (same coercion and document recording). Fails on the first bad write;
    /// writes already applied before it stand, since control writes have no
    /// rollback — validate keys with `list_controls` first if that matters.
    fn set_controls(&self, writes: &[ControlWrite]) -> Result<(), GraphCommandError> {
        for write in writes {
            self.set_control(&write.module_id, &write.key, write.value.clone())?;
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
/// Live runtimes route mutations through the audio-thread command queue, while
/// render runtimes apply the same commands directly to the in-memory graph.
#[derive(Clone)]
pub struct RuntimeController {
    pub(crate) snapshot: RuntimeSnapshot,
    pub(crate) registry: ModuleRegistry,
    pub(crate) sample_rate: u32,
    pub(crate) graph: Option<Arc<Mutex<SignalGraph>>>,
    pub(crate) command_tx: Option<mpsc::Sender<GraphCommand>>,
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

    /// Sets the current value of a module control, records it in the retained
    /// document so the change survives a save/rebuild, and announces it as a
    /// [`RpcEventPayload::ControlChanged`] carrying the *applied* value.
    ///
    /// This is the choke point for externally-initiated control writes — RPC
    /// commands, conducting scripts, and agents — so an observer sees every one
    /// of them (finding FUG-239 #7). Internal reconstruction that must stay
    /// silent (a reload carrying values into the rebuilt graph) uses
    /// [`Self::set_control_recorded`] instead.
    pub fn set_control(
        &self,
        module_id: &str,
        key: &str,
        value: ControlValue,
    ) -> Result<(), GraphCommandError> {
        let applied = self.set_control_recorded(module_id, key, value)?;
        self.emit_control_changed(module_id, key, applied);
        Ok(())
    }

    /// Coerces, applies, and records a control write without emitting an event,
    /// returning the applied (coerced) value. For internal callers that record
    /// a change but must not surface it as a live, agent-visible
    /// `ControlChanged` (e.g. a reload carrying authored values into a freshly
    /// rebuilt graph, which is already conveyed by the reload's snapshot).
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
        let value = {
            let controls = self.control_surfaces.lock().unwrap();
            match controls.get(module_id) {
                Some(surface) => surface.coerce_value(key, value),
                None => value,
            }
        };
        self.set_control_transient(module_id, key, value.clone())?;
        self.state
            .lock()
            .unwrap()
            .document_write_control(module_id, key, &value);
        Ok(value)
    }

    /// Announces a recorded control change to the installed event sink, if any.
    fn emit_control_changed(&self, module_id: &str, key: &str, value: ControlValue) {
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
    fn send_or_apply(&self, cmd: GraphCommand) -> Result<(), GraphCommandError> {
        if let Some(command_tx) = &self.command_tx {
            command_tx
                .send(cmd)
                .map_err(|_| GraphCommandError::AudioThreadStopped)
        } else {
            let graph = self
                .graph
                .as_ref()
                .ok_or(GraphCommandError::AudioThreadStopped)?;
            graph.lock().unwrap().apply_command(cmd);
            Ok(())
        }
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
        if !self.registry.has_type(module_type) {
            return Err(GraphCommandError::UnknownModuleType(
                module_type.to_string(),
            ));
        }

        let ModuleBuildResult {
            module,
            handles,
            control_surface,
            sink: _,
        } = self
            .registry
            .build(module_type, self.sample_rate, config)
            .map_err(|e| GraphCommandError::ModuleBuildFailed(e.to_string()))?;

        let ports = ModulePorts {
            inputs: module
                .module()
                .inputs()
                .iter()
                .map(|port| (*port).to_string())
                .collect(),
            outputs: module
                .module()
                .outputs()
                .iter()
                .map(|port| (*port).to_string())
                .collect(),
        };

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

        if let Some(control_surface) = control_surface {
            self.snapshot
                .control_surfaces
                .lock()
                .unwrap()
                .insert(module_id.to_string(), control_surface);
        }

        self.send_or_apply(GraphCommand::AddModule {
            module_id: module_id.to_string(),
            module,
        })?;

        self.module_ports
            .lock()
            .unwrap()
            .insert(module_id.to_string(), ports);

        {
            let mut state = self.snapshot.state.lock().unwrap();
            state.modules.insert(
                module_id.to_string(),
                RuntimeModuleInfo {
                    id: module_id.to_string(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
            state.document_upsert_module(module_id, module_type, config);
        }

        Ok(handles
            .into_iter()
            .map(|(name, handle)| (format!("{}.{}", module_id, name), handle))
            .collect())
    }

    /// Removes a module and any connections that reference it.
    pub fn remove_module(&self, module_id: &str) -> Result<(), GraphCommandError> {
        self.snapshot
            .control_surfaces
            .lock()
            .unwrap()
            .shift_remove(module_id);
        self.send_or_apply(GraphCommand::RemoveModule {
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

        self.send_or_apply(GraphCommand::AddConnection {
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
        self.send_or_apply(GraphCommand::RemoveConnection {
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
