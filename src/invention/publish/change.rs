//! Preparing a structural change off the audio thread.
//!
//! A [`GraphChange`] applies edits to a working copy of the publisher's
//! mirror, building instances and validating as it goes. Nothing visible
//! changes: not the audio graph, the runtime's mirrors, its control surfaces,
//! or its document. [`GraphChange::prepare`] then compiles the complete next
//! topology into a [`PreparedChange`] that only needs publishing.

use indexmap::IndexMap;
use std::any::Any;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::invention::declared::Route;
use crate::invention::graph::{
    compile_topology, vacant, Publication, RoutingConnection, SurvivorRemap, TopologyFacts,
};
use crate::invention::orchestration::ModulePorts;
use crate::invention::runtime::{ControlSurfaceInstance, GraphCommandError, ModuleInstance};
use crate::invention::state::RuntimeModuleInfo;
use crate::invention::timelines::{Context, Flow};
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
    pub(crate) flow: Flow,
    /// Numbers the module instance it mirrors: each build gets a fresh
    /// number, which a module keeps while it survives edits, so a module
    /// removed or rebuilt is told from the one now under its id (see
    /// `graph::taps`).
    pub(crate) instance: u32,
}

impl MirrorModule {
    pub(crate) fn of(instance: &ModuleInstance) -> Self {
        MirrorModule {
            ports: ports_of(instance),
            is_sink: matches!(instance, GraphModule::Sink(_)),
            flow: Flow::of(instance.module()),
            instance: next_instance(),
        }
    }
}

/// A module instance number no other instance has had in this process.
fn next_instance() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The authoritative control-thread mirror of a live graph's topology, in
/// the audio graph's exact module order.
#[derive(Clone, Debug, Default)]
pub(crate) struct TopologyMirror {
    pub(crate) modules: IndexMap<String, MirrorModule>,
    pub(crate) edges: Vec<RoutingConnection>,
    /// Each module's timeline context, in module order, as of the latest
    /// compile (see [`Self::settle_timelines`]).
    pub(crate) timelines: Vec<Context>,
    /// Ordering-only edges, source then target by id, that requests timed
    /// by a condition watch through: the source computes before the target
    /// so the request applies on its exact sample (see `graph::conditions`).
    /// Neither routed nor saved; dropped by the first change made once no
    /// condition waits (see [`GraphChange::forget_orders`]).
    pub(crate) watches: Vec<(String, String)>,
    /// Each module's control targets as the topology last published was
    /// compiled with them (see `change::order`).
    pub(crate) targets: order::Targets,
    /// The room for tables' ramps the drain holds once the topology last
    /// published installs (see `change::ramp_room`); 0 for the least.
    pub(crate) table_ramps: usize,
}

impl TopologyMirror {
    /// Mirrors the modules and edges of a graph about to go live.
    pub(crate) fn of(
        modules: &IndexMap<String, ModuleInstance>,
        edges: &[RoutingConnection],
    ) -> Self {
        let targets = modules
            .iter()
            .map(|(id, instance)| (id.clone(), instance.module().control_targets()));
        let targets = targets.collect();
        let modules = modules
            .iter()
            .map(|(id, instance)| (id.clone(), MirrorModule::of(instance)))
            .collect();
        let mut mirror = Self {
            modules,
            edges: edges.to_vec(),
            timelines: Vec::new(),
            watches: Vec::new(),
            targets,
            table_ramps: 0,
        };
        mirror.settle_timelines();
        mirror
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

    /// The instance key of each module, in module order: what a
    /// publication carries for the audio thread's output taps (see
    /// `crate::invention::graph::taps`).
    pub(crate) fn instance_keys(&self) -> Vec<u32> {
        self.modules
            .values()
            .map(|module| module.instance)
            .collect()
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
    /// Taken when the change is prepared (or by offline render's direct
    /// edits).
    pub(crate) instance: Option<ModuleInstance>,
    pub(crate) info: RuntimeModuleInfo,
    pub(crate) module: MirrorModule,
    pub(crate) surface: Option<ControlSurfaceInstance>,
    pub(crate) handles: Vec<(String, Arc<dyn Any + Send + Sync>)>,
    /// The control writes made through its setters once built, in order
    /// (an edit batch's to a module it builds), so the request log can
    /// make them again.
    pub(crate) writes: Vec<(String, crate::ControlValue)>,
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
    /// Modules an upsert replaced or a remove took out. They are kept, not
    /// dropped in place, so a change edited under the publisher lock can
    /// drop them after releasing it.
    discarded: Vec<BuiltModule>,
    /// What surviving schedulers re-resolve for.
    rescheduling: Rescheduling,
    /// Control targets a live schedule write leaves its scheduler writing
    /// (see [`Self::retarget`]), ahead of its surface reporting them.
    retargets: order::Targets,
    /// The most ramps at once a live schedule write leaves its scheduler
    /// running (see [`Self::write_ramps`]), ahead of its surface recording
    /// it.
    ramps_written: IndexMap<String, usize>,
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
            discarded: Vec::new(),
            rescheduling: Rescheduling::default(),
            retargets: order::Targets::new(),
            ramps_written: IndexMap::new(),
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
            .map_err(|e| GraphCommandError::ModuleBuildFailed(format!("module '{id}': {e}")))?;
        Ok(BuiltModule {
            module: MirrorModule {
                ports: ports_of(&result.module),
                is_sink: matches!(result.module, GraphModule::Sink(_)),
                flow: Flow::of(result.module.module()),
                instance: next_instance(),
            },
            instance: Some(result.module),
            info: RuntimeModuleInfo {
                id: id.to_string(),
                module_type: module_type.to_string(),
                config: config.clone(),
            },
            surface: result.control_surface,
            handles: result.handles,
            writes: Vec::new(),
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
        if let Some(replaced) = self.built.insert(id.to_string(), module) {
            self.discarded.push(replaced);
        }
        self.rescheduling.touched.insert(id.to_string());
        self.edits += 1;
    }

    /// Takes the modules this change's upserts and removes displaced, for
    /// the caller to drop where it chooses.
    pub(crate) fn take_discarded(&mut self) -> Vec<BuiltModule> {
        std::mem::take(&mut self.discarded)
    }

    /// Removes module `id` and its connections; a missing id is a no-op.
    pub(crate) fn remove(&mut self, id: &str) {
        if self.contains(id) {
            if let Some(removed) = self.built.shift_remove(id) {
                self.discarded.push(removed);
            }
            self.mirror.remove(id);
            self.surfaces.shift_remove(id);
            self.rescheduling.touched.insert(id.to_string());
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

    /// Removes every connection, for a change that then makes exactly the
    /// ones it keeps, in order.
    pub(crate) fn disconnect_all(&mut self) {
        if !self.mirror.edges.is_empty() {
            self.mirror.edges.clear();
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
    /// leave it, checks that surviving schedulers targeting a module the
    /// change rebuilds or removes still resolve (see [`Rescheduling`]),
    /// prepares each new instance for publication (see
    /// [`crate::Module::prepare_for_publication`]), then compiles the
    /// complete next topology. Fails, with nothing visible changed, when a
    /// schedule cannot resolve, a survivor's included.
    pub(crate) fn prepare(mut self) -> Result<PreparedChange, GraphCommandError> {
        self.attach()?;
        Ok(self.compile())
    }

    /// The fallible part of [`Self::prepare`]: attaches new schedulers and
    /// checks surviving ones, which the commit re-resolves again under the
    /// publisher.
    /// On failure the change, and every instance it built, is still the
    /// caller's to drop where it chooses.
    pub(crate) fn attach(&mut self) -> Result<(), GraphCommandError> {
        if self.edits == 0 {
            return Ok(());
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
        self.rescheduling
            .rebinds(&self.surfaces, &self.built)
            .map(drop)
    }

    /// The rest of [`Self::prepare`], once [`Self::attach`] has succeeded:
    /// prepares each new instance and compiles the next topology.
    pub(crate) fn compile(mut self) -> PreparedChange {
        if self.edits == 0 {
            // Nothing to compile or publish.
            return PreparedChange {
                base_generation: self.base_generation,
                mirror: self.mirror,
                surfaces: self.surfaces,
                built: self.built,
                rescheduling: Rescheduling::default(),
                publication: None,
                writes: Vec::new(),
                authored_at: None,
            };
        }
        // Attached, so each new instance can do its one-time setup here
        // rather than in its first block on the audio thread.
        // Writes made while building reach the instance here, while it is
        // still on this thread; the surface takes requests only once the
        // change commits (see `LiveGraph::commit_with`), and refuses writes
        // in between, or forever if the change never commits.
        for module in self.built.values_mut() {
            if let Some(instance) = module.instance.as_mut() {
                if let Some(surface) = &module.surface {
                    surface.bind(Route::Prepared, instance.module_mut());
                }
                instance.module_mut().prepare_for_publication();
            }
        }
        let mut instances: IndexMap<String, ModuleInstance> = self
            .built
            .iter_mut()
            .filter_map(|(id, module)| Some((id.clone(), module.instance.take()?)))
            .collect();
        // Read once: what the publication compiles with is what it holds.
        self.mirror.targets = self.targets_now();
        self.keep_orders_inaudible();
        self.mirror.settle_timelines();
        let ramp_room = self.grow_ramp_room();
        let mut publication = build_publication(&self.mirror, &mut instances, self.block_size);
        publication.ramp_room = ramp_room;
        PreparedChange {
            base_generation: self.base_generation,
            mirror: self.mirror,
            surfaces: self.surfaces,
            built: self.built,
            rescheduling: self.rescheduling,
            publication: Some(publication),
            writes: Vec::new(),
            authored_at: None,
        }
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
    /// The control-surface directory as the change will leave it, for
    /// validating control writes that land with it.
    pub(crate) surfaces: SurfaceMap,
    pub(crate) built: IndexMap<String, BuiltModule>,
    /// What surviving schedulers re-resolve for, which the commit does
    /// under the publisher.
    pub(crate) rescheduling: Rescheduling,
    /// The compiled next topology; `None` when the change edits nothing.
    pub(crate) publication: Option<Box<Publication>>,
    /// The controls its caller writes once it commits, as an edit batch or
    /// a reload does: an authored write to one, not yet recorded, is
    /// replaced (see `authored::Fence`), whatever value it writes.
    pub(crate) writes: Vec<(String, String)>,
    /// The retained document's authored epoch it was planned at, for a
    /// change that records a document planned from it (an edit batch's, a
    /// reload's), which lacks the authored writes recorded since: an
    /// untimed commit records them again on top of it, and a timed one is
    /// refused `TopologyMoved`, as is an untimed one once they are no
    /// longer all kept (see `RuntimeState::authored_log`).
    pub(crate) authored_at: Option<u64>,
}

impl PreparedChange {
    /// Whether the change edits nothing.
    pub(crate) fn is_empty(&self) -> bool {
        self.publication.is_none()
    }
}

/// Compilation facts read from a mirror, with each module's control
/// targets read from its surface beforehand (see [`order::Targets`]).
struct MirrorFacts<'a> {
    mirror: &'a TopologyMirror,
    targets: &'a order::Targets,
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
        let Some((id, _)) = self.mirror.modules.get_index(module) else {
            return Vec::new();
        };
        let mut targets = self.targets.get(id).cloned().unwrap_or_default();
        let watched = self
            .mirror
            .watches
            .iter()
            .filter(|(source, _)| source == id);
        targets.extend(watched.map(|(_, target)| target.clone()));
        targets
    }
}

/// Compiles `mirror` into a publication, taking the prepared instance for
/// each module from `instances` and a survivor placeholder otherwise.
fn build_publication(
    mirror: &TopologyMirror,
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
        &MirrorFacts {
            mirror,
            targets: &mirror.targets,
        },
        &mirror.edges,
        &sinks,
        block_size,
    );
    let inherited = vec![false; survivor.len()];
    Box::new(Publication {
        modules,
        survivor,
        inherited,
        sinks,
        edges: mirror.edges.clone(),
        topology,
        remap: SurvivorRemap::default(),
        generation: 0,
        handovers: Vec::new(),
        fate: None,
        ramp_room: None,
        note_taps: None,
        tap_keys: None,
    })
}

// Used by `publish::watch`, which the front doors FUG-317 builds submit to.
#[cfg_attr(not(test), allow(dead_code))]
mod offline;
mod order;
mod ramp_room;
mod rebind;
pub(crate) use offline::edit_offline;
pub(crate) use rebind::Rescheduling;
#[cfg(test)]
mod tests;
mod timelines;
