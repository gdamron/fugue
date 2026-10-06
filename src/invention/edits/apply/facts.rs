//! What a batch is checked against on a running invention: its modules'
//! ports and controls, and its current registry.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use indexmap::IndexMap;

use crate::invention::builder::resolve_invention_assets;
use crate::invention::edits::{EditFacts, ModuleFacts};
use crate::invention::format::{Invention, ModuleSpec};
use crate::invention::orchestration::ModulePorts;
use crate::invention::runtime::{ControlSurfaceInstance, RunningInvention};
use crate::modules::control_scheduler::name_unattached_from_handles;
use crate::traits::ControlSurfaceMap;
use crate::{ControlKind, ControlValue, ModuleRegistry};

/// The running invention's facts, read once when the batch starts, so every
/// edit is checked against the same view of the graph.
pub(super) struct LiveFacts {
    registry: Arc<ModuleRegistry>,
    sample_rate: u32,
    ports: IndexMap<String, ModulePorts>,
    surfaces: IndexMap<String, ControlSurfaceInstance>,
    /// The retained document with no modules: its assets and source path,
    /// which an added module's config is resolved against.
    assets: Invention,
    /// The control surface of each module `describe` built for the batch,
    /// by id; the latest build for an id replaces an earlier one. The
    /// batch's writes to an added module are made on it, in order.
    described: HashMap<String, Option<ControlSurfaceInstance>>,
    /// Each running module's type and config as authored, for building the
    /// throwaway copy a batch writes to.
    authored: HashMap<String, (String, serde_json::Value)>,
    /// A throwaway copy of each running module the batch writes, built in
    /// validation mode from its authored config, with the batch's writes so
    /// far made on it in order. The running module is never written before
    /// the commit.
    provisional: HashMap<String, ControlSurfaceInstance>,
    /// Running modules whose authored config no longer builds a copy (a
    /// sample file deleted since it loaded): their writes are checked on
    /// the running module instead.
    unbuildable: HashSet<String>,
}

impl LiveFacts {
    pub(super) fn new(running: &RunningInvention, document: &Invention) -> Self {
        let assets = Invention {
            modules: Vec::new(),
            connections: Vec::new(),
            developments: Vec::new(),
            ..document.clone()
        };
        let authored = document
            .modules
            .iter()
            .map(|spec| {
                let typed = (spec.module_type.clone(), spec.config.clone());
                (spec.id.clone(), typed)
            })
            .collect();
        Self {
            registry: running.registry(),
            sample_rate: running.sample_rate,
            ports: running.module_ports.lock().unwrap().clone(),
            surfaces: running.control_surfaces.lock().unwrap().clone(),
            assets,
            described: HashMap::new(),
            authored,
            provisional: HashMap::new(),
            unbuildable: HashSet::new(),
        }
    }

    /// The throwaway copies of the running modules the batch wrote, with
    /// every write made: what the later checks see for those modules.
    pub(super) fn into_provisional(self) -> ControlSurfaceMap {
        self.provisional.into_iter().collect()
    }

    /// The surface a write to `id` lands on at this point in the batch: the
    /// described instance of a module the batch added, or the throwaway copy
    /// of a running one, built on first use.
    fn writable(&mut self, id: &str) -> Result<ControlSurfaceInstance, String> {
        if let Some(surface) = self.described.get(id) {
            return surface
                .clone()
                .ok_or_else(|| "the module has no controls".to_string());
        }
        if let Some(surface) = self.provisional.get(id) {
            return Ok(surface.clone());
        }
        let (module_type, config) = self
            .authored
            .get(id)
            .cloned()
            .ok_or_else(|| "the module is not in the authored document".to_string())?;
        if self.unbuildable.contains(id) {
            return Err("the module's authored config does not build".to_string());
        }
        let built = self.resolve(id, &module_type, &config).and_then(|config| {
            self.registry
                .for_validation()
                .build(&module_type, self.sample_rate, &config)
                .map_err(|error| error.to_string())
        });
        let built = match built {
            Ok(built) => built,
            Err(error) => {
                if self.surfaces.contains_key(id) {
                    self.unbuildable.insert(id.to_string());
                }
                return Err(error);
            }
        };
        name_unattached_from_handles(id, &built.handles);
        let surface = built
            .control_surface
            .ok_or_else(|| "the module has no controls".to_string())?;
        restore_authored(surface.as_ref(), &config);
        self.provisional.insert(id.to_string(), surface.clone());
        Ok(surface)
    }

    /// The directory as the batch has it at this point: each of `modules`
    /// (the candidate's, at the edit being checked) as described, as its
    /// throwaway copy, or as running.
    fn directory_now(&self, modules: &[&str]) -> ControlSurfaceMap {
        modules
            .iter()
            .filter_map(|id| {
                let surface = match self.described.get(*id) {
                    Some(surface) => surface.clone(),
                    None => self
                        .provisional
                        .get(*id)
                        .or_else(|| self.surfaces.get(*id))
                        .cloned(),
                };
                Some((id.to_string(), surface?))
            })
            .collect()
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
                    None => self
                        .provisional
                        .get(&spec.id)
                        .or_else(|| self.surfaces.get(&spec.id))
                        .cloned(),
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

impl EditFacts for LiveFacts {
    fn module(&self, id: &str) -> Option<ModuleFacts> {
        let ports = self.ports.get(id)?;
        Some(ModuleFacts {
            inputs: ports.inputs.clone(),
            outputs: ports.outputs.clone(),
            controls: self
                .provisional
                .get(id)
                .or_else(|| self.surfaces.get(id))
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
        name_unattached_from_handles(id, &built.handles);
        let facts = ModuleFacts::from_instance(
            &built.module,
            built.control_surface.as_deref().map(|surface| surface as _),
        );
        self.described.insert(id.to_string(), built.control_surface);
        Ok(facts)
    }

    fn forget(&mut self, id: &str) {
        self.described.remove(id);
        self.provisional.remove(id);
    }

    fn authored_controls(&mut self, id: &str) -> Option<BTreeMap<String, ControlKind>> {
        let surface = self.writable(id).ok()?;
        let controls = surface
            .controls()
            .into_iter()
            .map(|meta| (meta.key, meta.kind))
            .collect();
        Some(controls)
    }

    fn write_control(
        &mut self,
        id: &str,
        key: &str,
        value: &ControlValue,
        modules: &[&str],
    ) -> Result<Option<BTreeMap<String, ControlKind>>, String> {
        let surface = match self.writable(id) {
            Ok(surface) => surface,
            // A running module whose authored config no longer builds (a
            // sample file deleted since it loaded) is checked as it runs,
            // write by write, without tracking what each write changes.
            Err(_) if self.unbuildable.contains(id) => {
                let live = self
                    .surfaces
                    .get(id)
                    .cloned()
                    .ok_or("the module has no controls")?;
                live.validate_control(key, value, &self.directory_now(modules))?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        // The module's own rules, and what only the directory can tell (a
        // schedule's targets), checked at this write, not just for the
        // control's final value.
        let mut directory = self.directory_now(modules);
        directory.insert(id.to_string(), surface.clone());
        surface.validate_control(key, value, &directory)?;
        // Then made on the copy, so the edits after it see what it changed.
        // A setter can still fail where the rules pass, on what is left to
        // the write itself (a sample that does not load): that is reported
        // when the write is made at commit, and the copy keeps its state.
        let _ = surface.set_control(key, value.clone());
        let controls = surface
            .controls()
            .into_iter()
            .map(|meta| (meta.key, meta.kind))
            .collect();
        Ok(Some(controls))
    }
}

/// Brings a throwaway copy built from `config` to the control values its
/// authored writes recorded there: a factory that does not read one of its
/// controls' keys from its config (a count that sizes other controls, say)
/// would otherwise leave the copy short of what the running module has.
/// Only values the build did not already give are set, unindexed keys (a
/// count) before indexed ones; a value the copy refuses is left as built.
fn restore_authored(surface: &dyn crate::ControlSurface, config: &serde_json::Value) {
    let Some(entries) = config.as_object() else {
        return;
    };
    let mut keys: Vec<&String> = entries.keys().collect();
    keys.sort_by_key(|key| key.contains('.'));
    for key in keys {
        let value = match &entries[key.as_str()] {
            serde_json::Value::Number(number) => {
                ControlValue::Number(number.as_f64().unwrap_or(0.0) as f32)
            }
            serde_json::Value::Bool(flag) => ControlValue::Bool(*flag),
            serde_json::Value::String(text) => ControlValue::String(text.clone()),
            _ => continue,
        };
        if !surface.controls().iter().any(|meta| &meta.key == key) {
            continue;
        }
        let value = surface.coerce_value(key, value);
        if surface.get_control(key).ok().as_ref() == Some(&value) {
            continue;
        }
        let _ = surface.set_control(key, value);
    }
}
