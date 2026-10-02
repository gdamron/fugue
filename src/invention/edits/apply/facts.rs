//! What a batch is checked against on a running invention: its modules'
//! ports and controls, and its current registry.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::invention::edits::{EditFacts, ModuleFacts};
use crate::invention::orchestration::ModulePorts;
use crate::invention::publish::{BuiltModule, GraphChange};
use crate::invention::runtime::{ControlSurfaceInstance, GraphCommandError, RunningInvention};
use crate::ModuleRegistry;

/// A module `describe` built for an `add_module`, kept so the commit can
/// reuse it rather than build the same module twice.
pub(super) struct Kept {
    /// The config the module was built from.
    config: serde_json::Value,
    module: BuiltModule,
}

/// Modules built while the batch was checked, by id.
#[derive(Default)]
pub(super) struct KeptModules(HashMap<String, Kept>);

impl KeptModules {
    /// Takes the module kept for `id` when it was built as `module_type`
    /// from `config`. One built otherwise is dropped: a later `set_control`
    /// changed what the module must be built from.
    pub(super) fn take(
        &mut self,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Option<BuiltModule> {
        let kept = self.0.remove(id)?;
        (kept.module.info.module_type == module_type && kept.config == *config)
            .then_some(kept.module)
    }
}

/// The running invention's facts, read once when the batch starts, so every
/// edit is checked against the same view of the graph.
pub(super) struct LiveFacts<'r> {
    registry: &'r ModuleRegistry,
    sample_rate: u32,
    ports: IndexMap<String, ModulePorts>,
    surfaces: IndexMap<String, ControlSurfaceInstance>,
    kept: KeptModules,
}

impl<'r> LiveFacts<'r> {
    pub(super) fn new(running: &'r RunningInvention) -> Self {
        Self {
            registry: &running.registry,
            sample_rate: running.sample_rate,
            ports: running.module_ports.lock().unwrap().clone(),
            surfaces: running.control_surfaces.lock().unwrap().clone(),
            kept: KeptModules::default(),
        }
    }

    /// The modules `describe` built and kept, for the commit to reuse.
    pub(super) fn into_kept(self) -> KeptModules {
        self.kept
    }
}

impl EditFacts for LiveFacts<'_> {
    fn module(&self, id: &str) -> Option<ModuleFacts> {
        let ports = self.ports.get(id)?;
        Some(ModuleFacts {
            inputs: ports.inputs.clone(),
            outputs: ports.outputs.clone(),
            controls: self
                .surfaces
                .get(id)
                .map(|surface| {
                    surface
                        .controls()
                        .into_iter()
                        .map(|meta| (meta.key, meta.kind))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    fn has_type(&self, module_type: &str) -> bool {
        self.registry.has_type(module_type)
    }

    /// Builds the module for real with the registry the commit builds with,
    /// and keeps it: the latest build for an id replaces an earlier one.
    fn describe(
        &mut self,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<ModuleFacts, String> {
        let module = GraphChange::build(self.registry, self.sample_rate, id, module_type, config)
            .map_err(|error| match error {
            GraphCommandError::ModuleBuildFailed(reason) => reason,
            other => other.to_string(),
        })?;
        let facts = ModuleFacts {
            inputs: module.module.ports.inputs.clone(),
            outputs: module.module.ports.outputs.clone(),
            controls: module
                .surface
                .as_ref()
                .map(|surface| {
                    surface
                        .controls()
                        .into_iter()
                        .map(|meta| (meta.key, meta.kind))
                        .collect()
                })
                .unwrap_or_default(),
        };
        let kept = Kept {
            config: config.clone(),
            module,
        };
        self.kept.0.insert(id.to_string(), kept);
        Ok(facts)
    }

    fn forget(&mut self, id: &str) {
        self.kept.0.remove(id);
    }
}
