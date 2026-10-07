//! Live graph edits on a running invention. Each structural edit is prepared
//! off the audio thread and published as one atomic topology change (see
//! [`crate::invention::publish`]); direct input writes are queued.

use super::{GraphCommandError, RunningInvention};
use crate::invention::graph::InputWrite;
use crate::invention::handles::InventionHandles;
use crate::invention::publish::{edge, Committed};

impl RunningInvention {
    /// Sets a module's input port to a specific value.
    ///
    /// The write is queued to the audio thread and applied at the start of
    /// the next block. This is fire-and-forget: if the module or port doesn't
    /// exist, the write is silently ignored on the audio thread. Fails when
    /// the audio thread is gone or has stopped draining writes.
    pub fn set_module_input(
        &self,
        module_id: impl Into<String>,
        port: impl Into<String>,
        value: f32,
    ) -> Result<(), GraphCommandError> {
        self.live.write_input(InputWrite {
            module_id: module_id.into(),
            port: port.into(),
            value,
        })
    }

    /// Adds a new module to the running graph.
    ///
    /// The module is built on the calling thread using the current registry
    /// (rebuilt if a reload adopts another first), then
    /// published to the audio thread. Handles are returned immediately (they
    /// use shared state internally and work regardless of graph state).
    ///
    /// If `module_id` already exists, the old module is replaced in place
    /// (hot-swap); connections to ports the new module lacks are dropped.
    pub fn add_module(
        &self,
        module_id: impl Into<String>,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<InventionHandles, GraphCommandError> {
        let module_id = module_id.into();
        let committed = self
            .live
            .add_module(self.sample_rate, &module_id, module_type, config)?;
        Ok(self.follow_up(committed))
    }

    /// Replaces a module in the running graph.
    ///
    /// The replacement module is built before the current module is touched, so
    /// unknown module types or invalid configs leave the running graph intact.
    /// When `preserve_connections` is true, connections touching the module
    /// whose ports the replacement also has are kept; the swap reaches the
    /// audio thread as one change.
    pub fn swap_module(
        &self,
        module_id: impl Into<String>,
        module_type: &str,
        config: &serde_json::Value,
        preserve_connections: bool,
    ) -> Result<InventionHandles, GraphCommandError> {
        let module_id = module_id.into();
        let committed = self.live.swap_module(
            self.sample_rate,
            &module_id,
            module_type,
            config,
            preserve_connections,
        )?;
        Ok(self.follow_up(committed))
    }

    /// Adds a connection between two modules in the running graph.
    ///
    /// Validates that both modules exist and have the specified ports before
    /// publishing. This gives callers immediate, actionable errors.
    pub fn connect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        self.live
            .connect(edge(from_module, from_port, to_module, to_port))
    }

    /// Disconnects two modules in the running graph. A connection that
    /// doesn't exist is a no-op.
    pub fn disconnect(
        &self,
        from_module: &str,
        from_port: &str,
        to_module: &str,
        to_port: &str,
    ) -> Result<(), GraphCommandError> {
        self.live
            .disconnect(edge(from_module, from_port, to_module, to_port))
    }

    /// Removes a module from the running graph, with every connection that
    /// references it. A module that doesn't exist is a no-op.
    pub fn remove_module(&self, module_id: impl Into<String>) -> Result<(), GraphCommandError> {
        let committed = self.live.remove_module(&module_id.into())?;
        self.follow_up(committed);
        Ok(())
    }

    /// Stops the scripts and agents of removed or replaced modules and starts
    /// those of newly built ones, returning the built modules' handles.
    pub(crate) fn follow_up(&self, committed: Committed) -> InventionHandles {
        for id in &committed.stopped {
            self.scripts.stop_module(id);
            self.agents.stop_module(id);
        }
        for info in committed.started {
            match info.module_type.as_str() {
                "code" => self.scripts.start_module(self.controller(), info),
                "agent" => self.agents.start_module(self.controller(), info),
                _ => {}
            }
        }
        InventionHandles::new(committed.handles)
    }
}
