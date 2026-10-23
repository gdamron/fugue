//! Graph edits on an offline render, made through the same change a live
//! edit makes and applied to the owned graph in place (see
//! [`edit_offline`]).

use std::sync::{Arc, Mutex};

use super::RenderEngine;
use crate::invention::graph::SignalGraph;
use crate::invention::publish::{edge, edit_offline, Committed, GraphChange};
use crate::invention::runtime::GraphCommandError;
use crate::InventionHandles;

impl RenderEngine {
    /// Adds a module, replacing one with the same id in place; connections
    /// to ports the new module lacks are dropped, and surviving schedulers
    /// that target it re-resolve against the new instance, as on a live
    /// graph.
    pub fn add_module(
        &self,
        module_id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<(), GraphCommandError> {
        let graph = self.loaded()?;
        let built = GraphChange::build(
            &self.registry,
            self.sample_rate,
            module_id,
            module_type,
            config,
        )?;
        let committed = self.edit(graph, |change| {
            change.upsert(module_id, built);
            Ok(())
        })?;
        self.follow_up(committed);
        Ok(())
    }

    /// Removes a module and its connections; a missing module is a no-op.
    /// Refused, with nothing changed, while a schedule still targets it.
    pub fn remove_module(&self, module_id: &str) -> Result<(), GraphCommandError> {
        let graph = self.loaded()?;
        let committed = self.edit(graph, |change| {
            change.remove(module_id);
            Ok(())
        })?;
        self.follow_up(committed);
        Ok(())
    }

    pub fn connect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        let graph = self.loaded()?;
        let edge = edge(from_module, from_port, to_module, to_port);
        self.edit(graph, |change| change.connect(edge)).map(drop)
    }

    pub fn disconnect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        let graph = self.loaded()?;
        let edge = edge(from_module, from_port, to_module, to_port);
        self.edit(graph, |change| {
            change.disconnect(edge);
            Ok(())
        })
        .map(drop)
    }

    fn loaded(&self) -> Result<&Arc<Mutex<SignalGraph>>, GraphCommandError> {
        self.graph
            .as_ref()
            .ok_or_else(|| GraphCommandError::ControlError("no invention loaded".to_string()))
    }

    fn edit(
        &self,
        graph: &Arc<Mutex<SignalGraph>>,
        edit: impl FnOnce(&mut GraphChange) -> Result<(), GraphCommandError>,
    ) -> Result<Committed, GraphCommandError> {
        let mirrors = (&self.control_surfaces, &*self.module_ports, &*self.state);
        edit_offline(graph, mirrors, false, edit)
    }

    /// Stops the scripts, agents and handles of removed or replaced modules
    /// and starts those of newly built ones, as a live edit does.
    fn follow_up(&self, committed: Committed) {
        let mut handles = self.handles.lock().unwrap();
        for id in &committed.stopped {
            self.scripts.stop_module(id);
            self.agents.stop_module(id);
            handles.remove_prefix(&format!("{id}."));
        }
        handles.merge(InventionHandles::new(committed.handles));
        drop(handles);
        for info in committed.started {
            match info.module_type.as_str() {
                "code" => self.scripts.start_module(
                    self.controller().expect("render controller available"),
                    info,
                ),
                "agent" => self.agents.start_module(
                    self.controller().expect("render controller available"),
                    info,
                ),
                _ => {}
            }
        }
    }
}
