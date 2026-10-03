//! What a batch is checked against on a running invention: its modules'
//! ports and controls, and its current registry.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::invention::builder::resolve_invention_assets;
use crate::invention::edits::{EditFacts, ModuleFacts};
use crate::invention::format::{Invention, ModuleSpec};
use crate::invention::orchestration::ModulePorts;
use crate::invention::runtime::{ControlSurfaceInstance, RunningInvention};
use crate::traits::ControlSurfaceMap;
use crate::ModuleRegistry;

/// The running invention's facts, read once when the batch starts, so every
/// edit is checked against the same view of the graph.
pub(super) struct LiveFacts<'r> {
    registry: &'r ModuleRegistry,
    sample_rate: u32,
    ports: IndexMap<String, ModulePorts>,
    surfaces: IndexMap<String, ControlSurfaceInstance>,
    /// The retained document with no modules: its assets and source path,
    /// which an added module's config is resolved against.
    assets: Invention,
    /// The control surface of each module `describe` built for the batch,
    /// by id; the latest build for an id replaces an earlier one.
    described: HashMap<String, Option<ControlSurfaceInstance>>,
}

impl<'r> LiveFacts<'r> {
    pub(super) fn new(running: &'r RunningInvention, document: &Invention) -> Self {
        let assets = Invention {
            modules: Vec::new(),
            connections: Vec::new(),
            developments: Vec::new(),
            ..document.clone()
        };
        Self {
            registry: &running.registry,
            sample_rate: running.sample_rate,
            ports: running.module_ports.lock().unwrap().clone(),
            surfaces: running.control_surfaces.lock().unwrap().clone(),
            assets,
            described: HashMap::new(),
        }
    }

    /// The control-surface directory as `candidate` leaves it, as far as
    /// the batch's checks can tell: the running modules it keeps, and the
    /// modules `describe` built for the ones it adds or replaces. A module
    /// a later edit rebuilds from a changed config is in it as described.
    pub(super) fn directory_after(&self, candidate: &Invention) -> ControlSurfaceMap {
        candidate
            .modules
            .iter()
            .filter_map(|spec| {
                let surface = match self.described.get(&spec.id) {
                    Some(surface) => surface.clone(),
                    None => self.surfaces.get(&spec.id).cloned(),
                };
                Some((spec.id.clone(), surface?))
            })
            .collect()
    }

    /// `config` with its `$asset` and audio asset references resolved as the
    /// document's own modules are: against the retained document's assets,
    /// relative to its source path.
    fn resolve(
        &self,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let mut probe = self.assets.clone();
        probe.modules.push(ModuleSpec {
            id: id.to_string(),
            module_type: module_type.to_string(),
            config: config.clone(),
        });
        let resolved = resolve_invention_assets(probe).map_err(|error| error.to_string())?;
        Ok(resolved
            .modules
            .into_iter()
            .next()
            .map(|spec| spec.config)
            .unwrap_or_default())
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

    /// Builds a throwaway instance of the module, from its config with its
    /// assets resolved, in validation mode: nothing it builds activates an
    /// output (a recording's file, say), so checking a batch never disturbs
    /// what is playing. Its surface is kept for [`Self::directory_after`];
    /// the commit builds the module again for real.
    fn describe(
        &mut self,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<ModuleFacts, String> {
        let config = self.resolve(id, module_type, config)?;
        let built = self
            .registry
            .for_validation()
            .build(module_type, self.sample_rate, &config)
            .map_err(|error| error.to_string())?;
        let facts = ModuleFacts::from_instance(
            &built.module,
            built.control_surface.as_deref().map(|surface| surface as _),
        );
        self.described.insert(id.to_string(), built.control_surface);
        Ok(facts)
    }

    fn forget(&mut self, id: &str) {
        self.described.remove(id);
    }
}
