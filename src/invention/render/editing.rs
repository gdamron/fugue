//! Graph edits on an offline render, applied directly to the owned graph.

use super::RenderEngine;
use crate::invention::graph::GraphCommand;
use crate::invention::orchestration::ModulePorts;
use crate::invention::runtime::{validate_input_port, validate_output_port, GraphCommandError};
use crate::invention::state::{RuntimeConnectionInfo, RuntimeModuleInfo};
use crate::InventionHandles;

impl RenderEngine {
    pub fn add_module(
        &self,
        module_id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<(), GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| GraphCommandError::ControlError("no invention loaded".to_string()))?
            .clone();

        if !self.registry.has_type(module_type) {
            return Err(GraphCommandError::UnknownModuleType(
                module_type.to_string(),
            ));
        }
        let result = self
            .registry
            .build(module_type, self.sample_rate, config)
            .map_err(|e| GraphCommandError::ModuleBuildFailed(e.to_string()))?;

        let mut new_handles = std::collections::HashMap::new();
        for (handle_name, handle) in result.handles {
            let key = format!("{}.{}", module_id, handle_name);
            new_handles.insert(key, handle);
        }

        // Attach schedulers before touching the graph, so a schedule that
        // fails to resolve leaves the loaded invention unchanged.
        if module_type == crate::modules::control_scheduler::CONTROL_SCHEDULER_TYPE_ID {
            crate::modules::control_scheduler::attach_from_handle(
                module_id,
                new_handles.get(&format!("{}.controls", module_id)),
                &self.control_surfaces,
            )
            .map_err(GraphCommandError::ModuleBuildFailed)?;
        }

        self.handles
            .lock()
            .unwrap()
            .merge(InventionHandles::new(new_handles));

        if let Some(control_surface) = result.control_surface {
            self.control_surfaces
                .lock()
                .unwrap()
                .insert(module_id.to_string(), control_surface);
        }

        self.module_ports.lock().unwrap().insert(
            module_id.to_string(),
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

        graph
            .lock()
            .unwrap()
            .apply_command(GraphCommand::AddModule {
                module_id: module_id.to_string(),
                module: result.module,
            });

        {
            let mut state = self.state.lock().unwrap();
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

        if module_type == "code" {
            self.scripts.start_module(
                self.controller().expect("render controller available"),
                RuntimeModuleInfo {
                    id: module_id.to_string(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }
        if module_type == "agent" {
            self.agents.start_module(
                self.controller().expect("render controller available"),
                RuntimeModuleInfo {
                    id: module_id.to_string(),
                    module_type: module_type.to_string(),
                    config: config.clone(),
                },
            );
        }

        Ok(())
    }

    pub fn remove_module(&self, module_id: &str) -> Result<(), GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| GraphCommandError::ControlError("no invention loaded".to_string()))?;
        self.scripts.stop_module(module_id);
        self.agents.stop_module(module_id);
        self.control_surfaces
            .lock()
            .unwrap()
            .shift_remove(module_id);
        self.handles
            .lock()
            .unwrap()
            .remove_prefix(&format!("{}.", module_id));
        self.module_ports.lock().unwrap().shift_remove(module_id);
        graph
            .lock()
            .unwrap()
            .apply_command(GraphCommand::RemoveModule {
                module_id: module_id.to_string(),
            });
        let mut state = self.state.lock().unwrap();
        state.modules.shift_remove(module_id);
        state
            .connections
            .retain(|conn| conn.from != module_id && conn.to != module_id);
        state.document_remove_module(module_id);
        Ok(())
    }

    pub fn connect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| GraphCommandError::ControlError("no invention loaded".to_string()))?;
        {
            let graph = graph.lock().unwrap();
            let source = graph
                .modules
                .get(from_module)
                .ok_or_else(|| GraphCommandError::UnknownModule(from_module.to_string()))?;
            validate_output_port(source, from_port)
                .map_err(|e| GraphCommandError::InvalidPort(e.to_string()))?;
            let dest = graph
                .modules
                .get(to_module)
                .ok_or_else(|| GraphCommandError::UnknownModule(to_module.to_string()))?;
            validate_input_port(dest, to_port)
                .map_err(|e| GraphCommandError::InvalidPort(e.to_string()))?;
        }
        graph
            .lock()
            .unwrap()
            .apply_command(GraphCommand::AddConnection {
                from_module: from_module.to_string(),
                from_port: from_port.to_string(),
                to_module: to_module.to_string(),
                to_port: to_port.to_string(),
            });
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

    pub fn disconnect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        let graph = self
            .graph
            .as_ref()
            .ok_or_else(|| GraphCommandError::ControlError("no invention loaded".to_string()))?;
        graph
            .lock()
            .unwrap()
            .apply_command(GraphCommand::RemoveConnection {
                from_module: from_module.to_string(),
                from_port: from_port.to_string(),
                to_module: to_module.to_string(),
                to_port: to_port.to_string(),
            });
        self.state.lock().unwrap().connections.retain(|conn| {
            !(conn.from == from_module
                && conn.from_port == from_port
                && conn.to == to_module
                && conn.to_port == to_port)
        });
        Ok(())
    }
}
