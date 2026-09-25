//! Live graph edits on a running invention: modules, connections, and
//! direct input writes, queued to the audio thread.

use std::collections::HashMap;

use super::{GraphCommandError, RunningInvention};
use crate::invention::graph::GraphCommand;
use crate::invention::handles::InventionHandles;
use crate::invention::orchestration::ModulePorts;
use crate::invention::state::{RuntimeConnectionInfo, RuntimeModuleInfo};

impl RunningInvention {
    /// Sends a command to the audio thread for graph mutation.
    pub(crate) fn send_command(&self, cmd: GraphCommand) -> Result<(), GraphCommandError> {
        self.command_tx
            .send(cmd)
            .map_err(|_| GraphCommandError::AudioThreadStopped)
    }

    /// Sets a module's input port to a specific value.
    ///
    /// The command is sent to the audio thread and applied at the start of the
    /// next sample. This is fire-and-forget: if the module or port doesn't exist,
    /// the command is silently ignored on the audio thread.
    pub fn set_module_input(
        &self,
        module_id: impl Into<String>,
        port: impl Into<String>,
        value: f32,
    ) -> Result<(), GraphCommandError> {
        self.send_command(GraphCommand::SetModuleInput {
            module_id: module_id.into(),
            port: port.into(),
            value,
        })
    }

    /// Adds a new module to the running graph.
    ///
    /// The module is built on the main thread using the registry, then sent
    /// to the audio thread via the command queue. Handles are returned immediately
    /// (they use `Arc<Mutex<T>>` internally and work regardless of graph state).
    ///
    /// If `module_id` already exists, the old module is replaced (hot-swap).
    pub fn add_module(
        &self,
        module_id: impl Into<String>,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<InventionHandles, GraphCommandError> {
        if !self.registry.has_type(module_type) {
            return Err(GraphCommandError::UnknownModuleType(
                module_type.to_string(),
            ));
        }

        let result = self
            .registry
            .build(module_type, self.sample_rate, config)
            .map_err(|e| GraphCommandError::ModuleBuildFailed(e.to_string()))?;

        let module_id = module_id.into();

        // Collect handles with flat keys: "module_id.handle_name"
        let mut handle_map = HashMap::new();
        for (handle_name, handle) in result.handles {
            let key = format!("{}.{}", module_id, handle_name);
            handle_map.insert(key, handle);
        }

        // Attach schedulers before touching the graph, so a schedule that
        // fails to resolve leaves the running invention unchanged.
        if module_type == crate::modules::control_scheduler::CONTROL_SCHEDULER_TYPE_ID {
            crate::modules::control_scheduler::attach_from_handle(
                &module_id,
                handle_map.get(&format!("{}.controls", module_id)),
                &self.control_surfaces,
            )
            .map_err(GraphCommandError::ModuleBuildFailed)?;
        }

        if let Some(control_surface) = result.control_surface {
            self.control_surfaces
                .lock()
                .unwrap()
                .insert(module_id.clone(), control_surface);
        }

        self.module_ports.lock().unwrap().insert(
            module_id.clone(),
            ModulePorts {
                inputs: result
                    .module
                    .module()
                    .inputs()
                    .iter()
                    .map(|port| (*port).to_string())
                    .collect(),
                outputs: result
                    .module
                    .module()
                    .outputs()
                    .iter()
                    .map(|port| (*port).to_string())
                    .collect(),
            },
        );

        self.send_command(GraphCommand::AddModule {
            module_id: module_id.clone(),
            module: result.module,
        })?;

        {
            let mut state = self.state.lock().unwrap();
            state.modules.insert(
                module_id.clone(),
                RuntimeModuleInfo {
                    id: module_id.clone(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
            state.document_upsert_module(&module_id, module_type, config);
        }

        if module_type == "code" {
            self.scripts.start_module(
                self.controller(),
                RuntimeModuleInfo {
                    id: module_id.clone(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }
        if module_type == "agent" {
            self.agents.start_module(
                self.controller(),
                RuntimeModuleInfo {
                    id: module_id.clone(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }

        Ok(InventionHandles::new(handle_map))
    }

    /// Replaces a module in the running graph.
    ///
    /// The replacement module is built before the current module is touched, so
    /// unknown module types or invalid configs leave the running graph intact.
    /// When `preserve_connections` is true, compatible connections touching the
    /// module are restored after the replacement is queued.
    pub fn swap_module(
        &self,
        module_id: impl Into<String>,
        module_type: &str,
        config: &serde_json::Value,
        preserve_connections: bool,
    ) -> Result<InventionHandles, GraphCommandError> {
        if !self.registry.has_type(module_type) {
            return Err(GraphCommandError::UnknownModuleType(
                module_type.to_string(),
            ));
        }

        let result = self
            .registry
            .build(module_type, self.sample_rate, config)
            .map_err(|e| GraphCommandError::ModuleBuildFailed(e.to_string()))?;

        let module_id = module_id.into();
        if !self.state.lock().unwrap().modules.contains_key(&module_id) {
            return Err(GraphCommandError::UnknownModule(module_id));
        }

        let mut handle_map = HashMap::new();
        for (handle_name, handle) in result.handles {
            let key = format!("{}.{}", module_id, handle_name);
            handle_map.insert(key, handle);
        }

        // Attach schedulers before touching the graph, so a schedule that
        // fails to resolve leaves the running invention unchanged.
        if module_type == crate::modules::control_scheduler::CONTROL_SCHEDULER_TYPE_ID {
            crate::modules::control_scheduler::attach_from_handle(
                &module_id,
                handle_map.get(&format!("{}.controls", module_id)),
                &self.control_surfaces,
            )
            .map_err(GraphCommandError::ModuleBuildFailed)?;
        }

        let ports = ModulePorts {
            inputs: result
                .module
                .module()
                .inputs()
                .iter()
                .map(|port| (*port).to_string())
                .collect(),
            outputs: result
                .module
                .module()
                .outputs()
                .iter()
                .map(|port| (*port).to_string())
                .collect(),
        };

        let related_connections: Vec<RuntimeConnectionInfo> = self
            .state
            .lock()
            .unwrap()
            .connections
            .iter()
            .filter(|conn| conn.from == module_id || conn.to == module_id)
            .cloned()
            .collect();
        let preserved_connections: Vec<RuntimeConnectionInfo> = if preserve_connections {
            related_connections
                .iter()
                .filter(|conn| {
                    let output_ok = conn.from != module_id
                        || ports.outputs.iter().any(|port| port == &conn.from_port);
                    let input_ok = conn.to != module_id
                        || ports.inputs.iter().any(|port| port == &conn.to_port);
                    output_ok && input_ok
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        for conn in &related_connections {
            self.send_command(GraphCommand::RemoveConnection {
                from_module: conn.from.clone(),
                from_port: conn.from_port.clone(),
                to_module: conn.to.clone(),
                to_port: conn.to_port.clone(),
            })?;
        }
        self.send_command(GraphCommand::AddModule {
            module_id: module_id.clone(),
            module: result.module,
        })?;
        for conn in &preserved_connections {
            self.send_command(GraphCommand::AddConnection {
                from_module: conn.from.clone(),
                from_port: conn.from_port.clone(),
                to_module: conn.to.clone(),
                to_port: conn.to_port.clone(),
            })?;
        }

        self.scripts.stop_module(&module_id);
        self.agents.stop_module(&module_id);
        if let Some(control_surface) = result.control_surface {
            self.control_surfaces
                .lock()
                .unwrap()
                .insert(module_id.clone(), control_surface);
        } else {
            self.control_surfaces
                .lock()
                .unwrap()
                .shift_remove(&module_id);
        }
        self.module_ports
            .lock()
            .unwrap()
            .insert(module_id.clone(), ports);

        let mut state = self.state.lock().unwrap();
        state.modules.insert(
            module_id.clone(),
            RuntimeModuleInfo {
                id: module_id.clone(),
                module_type: module_type.to_string(),
                config: config.clone(),
            },
        );
        state
            .connections
            .retain(|conn| conn.from != module_id && conn.to != module_id);
        state.connections.extend(preserved_connections);
        state.document_upsert_module(&module_id, module_type, config);
        drop(state);

        if module_type == "code" {
            self.scripts.start_module(
                self.controller(),
                RuntimeModuleInfo {
                    id: module_id.clone(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }
        if module_type == "agent" {
            self.agents.start_module(
                self.controller(),
                RuntimeModuleInfo {
                    id: module_id.clone(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }

        Ok(InventionHandles::new(handle_map))
    }

    /// Adds a connection between two modules in the running graph.
    ///
    /// Validates that both modules exist and have the specified ports before
    /// sending the command to the audio thread. This gives callers immediate,
    /// actionable errors.
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

        self.send_command(GraphCommand::AddConnection {
            from_module: from_module.to_string(),
            from_port: from_port.to_string(),
            to_module: to_module.to_string(),
            to_port: to_port.to_string(),
        })?;

        self.state
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

    /// Disconnects two modules in the running graph.
    ///
    /// This is fire-and-forget: if the connection doesn't exist, the command is
    /// silently ignored on the audio thread.
    pub fn disconnect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        self.send_command(GraphCommand::RemoveConnection {
            from_module: from_module.to_string(),
            from_port: from_port.to_string(),
            to_module: to_module.to_string(),
            to_port: to_port.to_string(),
        })?;

        self.state.lock().unwrap().connections.retain(|conn| {
            !(conn.from == from_module
                && conn.from_port == from_port
                && conn.to == to_module
                && conn.to_port == to_port)
        });

        Ok(())
    }

    /// Removes a module from the running graph.
    ///
    /// This is fire-and-forget: if the module doesn't exist, the command is
    /// silently ignored on the audio thread. All connections referencing the
    /// removed module are cleaned up.
    pub fn remove_module(&self, module_id: impl Into<String>) -> Result<(), GraphCommandError> {
        let module_id = module_id.into();
        self.scripts.stop_module(&module_id);
        self.agents.stop_module(&module_id);
        self.control_surfaces
            .lock()
            .unwrap()
            .shift_remove(&module_id);
        self.module_ports.lock().unwrap().shift_remove(&module_id);
        self.send_command(GraphCommand::RemoveModule {
            module_id: module_id.clone(),
        })?;
        let mut state = self.state.lock().unwrap();
        state.modules.shift_remove(&module_id);
        state
            .connections
            .retain(|conn| conn.from != module_id && conn.to != module_id);
        state.document_remove_module(&module_id);
        Ok(())
    }
}
