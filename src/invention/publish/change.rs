//! Preparing a structural change off the audio thread.
//!
//! A [`GraphChange`] applies edits to a working copy of the publisher's
//! mirror, building instances and validating as it goes. Nothing visible
//! changes: not the audio graph, the runtime's mirrors, its control surfaces,
//! or its document. [`GraphChange::prepare`] then compiles the complete next
//! topology into a [`PreparedChange`] that only needs publishing.

use indexmap::IndexMap;
use std::any::Any;
use std::sync::Arc;

use crate::invention::graph::{
    compile_topology, vacant, Publication, RoutingConnection, TopologyFacts,
};
use crate::invention::orchestration::ModulePorts;
use crate::invention::runtime::{ControlSurfaceInstance, GraphCommandError, ModuleInstance};
use crate::invention::state::RuntimeModuleInfo;
use crate::modules::control_scheduler::{
    attach_from_handle_resolving, SurfaceDirectory, CONTROL_SCHEDULER_TYPE_ID,
};
use crate::{GraphModule, ModuleRegistry};

/// Control-surface directory contents, keyed by module id.
pub(crate) type SurfaceMap = IndexMap<String, ControlSurfaceInstance>;

/// The publisher's view of one module: the facts topology compilation needs.
#[derive(Clone, Debug)]
pub(crate) struct MirrorModule {
    pub(crate) ports: ModulePorts,
    pub(crate) is_sink: bool,
}

/// The authoritative control-thread mirror of a live graph's topology, in
/// the audio graph's exact module order.
#[derive(Clone, Debug, Default)]
pub(crate) struct TopologyMirror {
    pub(crate) modules: IndexMap<String, MirrorModule>,
    pub(crate) edges: Vec<RoutingConnection>,
}

impl TopologyMirror {
    /// Mirrors the modules and edges of a graph about to go live.
    pub(crate) fn of(
        modules: &IndexMap<String, ModuleInstance>,
        edges: &[RoutingConnection],
    ) -> Self {
        let modules = modules
            .iter()
            .map(|(id, instance)| {
                let module = MirrorModule {
                    ports: ports_of(instance),
                    is_sink: matches!(instance, GraphModule::Sink(_)),
                };
                (id.clone(), module)
            })
            .collect();
        Self {
            modules,
            edges: edges.to_vec(),
        }
    }

    /// Sink module ids, in module order.
    pub(crate) fn sinks(&self) -> Vec<String> {
        self.modules
            .iter()
            .filter(|(_, module)| module.is_sink)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Adds a module, or replaces one in place, dropping edges that touch it
    /// through ports the new module does not have.
    fn upsert(&mut self, id: &str, module: MirrorModule) {
        let ports = &module.ports;
        self.edges.retain(|edge| {
            (edge.from_module != id || ports.outputs.contains(&edge.from_port))
                && (edge.to_module != id || ports.inputs.contains(&edge.to_port))
        });
        self.modules.insert(id.to_string(), module);
    }

    /// Removes a module and every edge touching it.
    fn remove(&mut self, id: &str) {
        self.modules.shift_remove(id);
        self.edges
            .retain(|edge| edge.from_module != id && edge.to_module != id);
    }

    /// Checks that both ends of `edge` exist with the named ports.
    fn check_edge(&self, edge: &RoutingConnection) -> Result<(), GraphCommandError> {
        let source = self
            .modules
            .get(&edge.from_module)
            .ok_or_else(|| GraphCommandError::UnknownModule(edge.from_module.clone()))?;
        if !source.ports.outputs.contains(&edge.from_port) {
            return Err(GraphCommandError::InvalidPort(format!(
                "module '{}' does not have output port '{}' (available: {:?})",
                edge.from_module, edge.from_port, source.ports.outputs
            )));
        }
        let dest = self
            .modules
            .get(&edge.to_module)
            .ok_or_else(|| GraphCommandError::UnknownModule(edge.to_module.clone()))?;
        if !dest.ports.inputs.contains(&edge.to_port) {
            return Err(GraphCommandError::InvalidPort(format!(
                "module '{}' does not have input port '{}' (available: {:?})",
                edge.to_module, edge.to_port, dest.ports.inputs
            )));
        }
        Ok(())
    }
}

/// Port names of a built instance.
pub(crate) fn ports_of(instance: &ModuleInstance) -> ModulePorts {
    let module = instance.module();
    ModulePorts {
        inputs: module.inputs().iter().map(|p| (*p).to_string()).collect(),
        outputs: module.outputs().iter().map(|p| (*p).to_string()).collect(),
    }
}

/// A module built for a change: its instance (until prepared), what the
/// mirrors record for it, and what it hands back to the caller.
pub(crate) struct BuiltModule {
    instance: Option<ModuleInstance>,
    pub(crate) info: RuntimeModuleInfo,
    pub(crate) module: MirrorModule,
    pub(crate) surface: Option<ControlSurfaceInstance>,
    pub(crate) handles: Vec<(String, Arc<dyn Any + Send + Sync>)>,
}

/// A structural change being prepared against a snapshot of the mirror.
pub(crate) struct GraphChange {
    pub(crate) base_generation: u64,
    mirror: TopologyMirror,
    surfaces: SurfaceMap,
    directory: SurfaceDirectory,
    /// Edits that changed the working topology.
    edits: usize,
    built: IndexMap<String, BuiltModule>,
    block_size: usize,
}

impl GraphChange {
    pub(crate) fn new(
        base_generation: u64,
        mirror: TopologyMirror,
        directory: SurfaceDirectory,
        block_size: usize,
    ) -> Self {
        let surfaces = directory.lock().unwrap().clone();
        Self {
            base_generation,
            mirror,
            surfaces,
            directory,
            edits: 0,
            built: IndexMap::new(),
            block_size,
        }
    }

    /// Whether the working topology has a module `id`.
    pub(crate) fn contains(&self, id: &str) -> bool {
        self.mirror.modules.contains_key(id)
    }

    /// Builds a module instance without adding it.
    pub(crate) fn build(
        registry: &ModuleRegistry,
        sample_rate: u32,
        id: &str,
        module_type: &str,
        config: &serde_json::Value,
    ) -> Result<BuiltModule, GraphCommandError> {
        if !registry.has_type(module_type) {
            return Err(GraphCommandError::UnknownModuleType(
                module_type.to_string(),
            ));
        }
        let result = registry
            .build(module_type, sample_rate, config)
            .map_err(|e| GraphCommandError::ModuleBuildFailed(e.to_string()))?;
        Ok(BuiltModule {
            module: MirrorModule {
                ports: ports_of(&result.module),
                is_sink: matches!(result.module, GraphModule::Sink(_)),
            },
            instance: Some(result.module),
            info: RuntimeModuleInfo {
                id: id.to_string(),
                module_type: module_type.to_string(),
                config: config.clone(),
            },
            surface: result.control_surface,
            handles: result.handles,
        })
    }

    /// Adds `module` as `id`, or replaces the module with that id in place.
    /// Connections to ports the new module lacks are dropped.
    pub(crate) fn upsert(&mut self, id: &str, module: BuiltModule) {
        self.mirror.upsert(id, module.module.clone());
        match &module.surface {
            Some(surface) => {
                self.surfaces.insert(id.to_string(), surface.clone());
            }
            None => {
                self.surfaces.shift_remove(id);
            }
        }
        self.built.insert(id.to_string(), module);
        self.edits += 1;
    }

    /// Removes module `id` and its connections; a missing id is a no-op.
    pub(crate) fn remove(&mut self, id: &str) {
        if self.contains(id) {
            self.built.shift_remove(id);
            self.mirror.remove(id);
            self.surfaces.shift_remove(id);
            self.edits += 1;
        }
    }

    /// Connects an output port to an input port after validating both ends.
    pub(crate) fn connect(&mut self, edge: RoutingConnection) -> Result<(), GraphCommandError> {
        self.mirror.check_edge(&edge)?;
        self.mirror.edges.push(edge);
        self.edits += 1;
        Ok(())
    }

    /// Removes a connection if present.
    pub(crate) fn disconnect(&mut self, edge: RoutingConnection) {
        let before = self.mirror.edges.len();
        self.mirror.edges.retain(|e| !same_edge(e, &edge));
        if self.mirror.edges.len() != before {
            self.edits += 1;
        }
    }

    /// Removes every connection touching module `id`.
    pub(crate) fn disconnect_module(&mut self, id: &str) {
        let touching: Vec<RoutingConnection> = self
            .mirror
            .edges
            .iter()
            .filter(|e| e.from_module == id || e.to_module == id)
            .cloned()
            .collect();
        for edge in touching {
            self.disconnect(edge);
        }
    }

    /// Attaches new schedulers against the directory as the change will
    /// leave it, prepares each new instance for publication (see
    /// [`crate::Module::prepare_for_publication`]), then compiles the
    /// complete next topology. Fails, with nothing visible changed, when a
    /// schedule cannot resolve.
    pub(crate) fn prepare(mut self) -> Result<PreparedChange, GraphCommandError> {
        if self.edits == 0 {
            // Nothing to attach, compile, or publish.
            return Ok(PreparedChange {
                base_generation: self.base_generation,
                mirror: self.mirror,
                built: self.built,
                publication: None,
            });
        }
        for (id, module) in &self.built {
            if module.info.module_type != CONTROL_SCHEDULER_TYPE_ID {
                continue;
            }
            let handle = module
                .handles
                .iter()
                .find(|(name, _)| name == "controls")
                .map(|(_, handle)| handle);
            attach_from_handle_resolving(id, handle, &self.directory, &self.surfaces)
                .map_err(GraphCommandError::ModuleBuildFailed)?;
        }
        // Attached, so each new instance can do its one-time setup here
        // rather than in its first block on the audio thread.
        for module in self.built.values_mut() {
            if let Some(instance) = module.instance.as_mut() {
                instance.module_mut().prepare_for_publication();
            }
        }
        let mut instances: IndexMap<String, ModuleInstance> = self
            .built
            .iter_mut()
            .filter_map(|(id, module)| Some((id.clone(), module.instance.take()?)))
            .collect();
        let publication = build_publication(
            &self.mirror,
            &self.surfaces,
            &mut instances,
            self.block_size,
        );
        Ok(PreparedChange {
            base_generation: self.base_generation,
            mirror: self.mirror,
            built: self.built,
            publication: Some(publication),
        })
    }
}

fn same_edge(a: &RoutingConnection, b: &RoutingConnection) -> bool {
    a.from_module == b.from_module
        && a.from_port == b.from_port
        && a.to_module == b.to_module
        && a.to_port == b.to_port
}

/// A change ready to publish: the next mirror and the compiled publication,
/// valid only on top of the publication it was prepared against.
pub(crate) struct PreparedChange {
    /// The publisher generation the change was prepared against; a newer
    /// one makes the change stale, and the publisher refuses it.
    pub(crate) base_generation: u64,
    pub(crate) mirror: TopologyMirror,
    pub(crate) built: IndexMap<String, BuiltModule>,
    /// The compiled next topology; `None` when the change edits nothing.
    pub(crate) publication: Option<Box<Publication>>,
}

impl PreparedChange {
    /// Whether the change edits nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.publication.is_none()
    }
}

/// Compilation facts read from a mirror; control targets come from the
/// modules' control surfaces.
struct MirrorFacts<'a> {
    mirror: &'a TopologyMirror,
    surfaces: &'a SurfaceMap,
}

impl TopologyFacts for MirrorFacts<'_> {
    fn module_count(&self) -> usize {
        self.mirror.modules.len()
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.mirror.modules.get_index_of(id)
    }

    fn input_index(&self, module: usize, port: &str) -> Option<usize> {
        let (_, m) = self.mirror.modules.get_index(module)?;
        m.ports.inputs.iter().position(|p| p == port)
    }

    fn output_index(&self, module: usize, port: &str) -> Option<usize> {
        let (_, m) = self.mirror.modules.get_index(module)?;
        m.ports.outputs.iter().position(|p| p == port)
    }

    fn output_count(&self, module: usize) -> usize {
        self.mirror
            .modules
            .get_index(module)
            .map(|(_, m)| m.ports.outputs.len())
            .unwrap_or(0)
    }

    fn control_targets(&self, module: usize) -> Vec<String> {
        self.mirror
            .modules
            .get_index(module)
            .and_then(|(id, _)| self.surfaces.get(id))
            .map(|surface| surface.control_targets())
            .unwrap_or_default()
    }
}

/// Compiles `mirror` into a publication, taking the prepared instance for
/// each module from `instances` and a survivor placeholder otherwise.
fn build_publication(
    mirror: &TopologyMirror,
    surfaces: &SurfaceMap,
    instances: &mut IndexMap<String, ModuleInstance>,
    block_size: usize,
) -> Box<Publication> {
    let n = mirror.modules.len();
    let mut modules = IndexMap::with_capacity(n);
    let mut survivor = Vec::with_capacity(n);
    for id in mirror.modules.keys() {
        match instances.shift_remove(id) {
            Some(instance) => {
                modules.insert(id.clone(), instance);
                survivor.push(false);
            }
            None => {
                modules.insert(id.clone(), vacant());
                survivor.push(true);
            }
        }
    }
    let sinks = mirror.sinks();
    let topology = compile_topology(
        &MirrorFacts { mirror, surfaces },
        &mirror.edges,
        &sinks,
        block_size,
    );
    Box::new(Publication {
        modules,
        survivor,
        sinks,
        edges: mirror.edges.clone(),
        topology,
    })
}
