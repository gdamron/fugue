use crate::control_request::{ControlCells, ControlIndex, ControlTable, Refusal, RtValue};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::graph::RoutingConnection;
use crate::invention::runtime::{
    validate_input_port, validate_output_port, ControlSurfaceInstance, InventionRuntime,
};
use crate::payload::{Payload, Retired};
use crate::{
    ControlMeta, ControlSurface, ControlValue, Invention, Module, ModuleRegistry, MAX_BLOCK,
};
use indexmap::IndexMap;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use super::builder::InventionBuilder;
use super::load_warnings;
use super::reload::LoadedDevelopments;
use super::timelines::Enclosure;

#[cfg(test)]
mod build_count_tests;
mod compiled_graph;
mod config;
mod control_surface;
mod declared_controls;
#[cfg(test)]
mod declared_tests;
#[cfg(test)]
mod exact_tests;
mod fan_out;
#[cfg(test)]
mod fan_out_tests;
#[cfg(test)]
mod feedback_tests;
#[cfg(test)]
mod instrument_fan_out_tests;
#[cfg(test)]
mod instrument_note_off_tests;
#[cfg(test)]
mod kit_fan_out_tests;
#[cfg(test)]
mod lost_tests;
#[cfg(test)]
mod order_tests;
#[cfg(test)]
mod payload_tests;
mod process;
#[cfg(test)]
mod ramp_tests;
mod ranges;
#[cfg(test)]
mod ranges_tests;
mod replacing;
mod retarget;
#[cfg(test)]
mod sample_player_tests;
mod starting_values;
#[cfg(test)]
mod starting_values_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod unwind_tests;

use compiled_graph::{
    enclosure_of, module_indexes_slice, routes_of, unique_port_names, CompiledDevelopmentGraph,
    ExternalInputRoute, ExternalOutputRoute, Grouping,
};
use control_surface::DevelopmentControlSurface;
use declared_controls::{enclose, DevelopmentControls};
use retarget::{apply_reordered, InnerOrder};

pub(crate) struct DevelopmentFactory {
    pub(crate) name: String,
    pub(crate) definition: Invention,
    pub(crate) registry: ModuleRegistry,
    pub(crate) registered: Arc<Mutex<HashSet<String>>>,
    /// The developments `definition` declares, as loaded with the outermost
    /// document: nested developments build from these, never from disk again.
    pub(crate) loaded: Arc<LoadedDevelopments>,
    /// Each ranged control `definition` starts outside its range, with the
    /// value it starts at instead (see `starting_values::with_held_starts`):
    /// found once, by building the definition, and shared by the factories
    /// rebuilt from this one.
    pub(crate) held_starts: Arc<OnceLock<Vec<(String, f32)>>>,
    /// The exposed controls an alias of which fires an event, or `None`
    /// when its modules cannot say (see `starting_values::event_names`):
    /// found once, without building the definition, and shared by the
    /// factories rebuilt from this one.
    pub(crate) events: Arc<OnceLock<Option<HashSet<String>>>>,
}

impl DevelopmentFactory {
    /// This factory building from `registry`, with a copy of the developments
    /// registered so far. The copy is taken before the build, not during it:
    /// a development used inside this one shares the set, and locks it as it
    /// builds.
    fn rebuilt_with(&self, registry: ModuleRegistry) -> Self {
        let registered = self.registered.lock().unwrap().clone();
        Self {
            name: self.name.clone(),
            definition: self.definition.clone(),
            registry,
            registered: Arc::new(Mutex::new(registered)),
            loaded: self.loaded.clone(),
            held_starts: self.held_starts.clone(),
            events: self.events.clone(),
        }
    }
}

impl ModuleFactory for DevelopmentFactory {
    fn type_id(&self) -> &'static str {
        "__development__"
    }

    fn config_keys(&self) -> &'static [crate::module_config::ConfigKey] {
        replacing::config_keys(&self.definition, &self.registry)
    }

    #[allow(private_interfaces)]
    fn development(&self) -> Option<&DevelopmentFactory> {
        Some(self)
    }

    fn build_for_inspection(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        self.rebuilt_with(self.registry.for_inspection())
            .build(sample_rate, config)
    }

    fn build_for_validation(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        self.rebuilt_with(self.registry.for_validation())
            .build(sample_rate, config)
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        // A definition starting a ranged control outside its range starts
        // it within, as a configured value outside it would.
        let started = starting_values::with_held_starts(self, sample_rate, config);
        let ((module, control_surface, inner), folded) =
            starting_values::build_started(self, sample_rate, &started)?;
        let surface = control_surface.as_ref();
        let refused = config::apply_development_config(&self.name, &started, surface, &folded)?;
        // A value passed down to a nested development that sets it aside
        // is set aside here too: said once, here, as the instance's own.
        let passed_down = |warning: &load_warnings::Warning| {
            let [module] = warning.modules.as_slice() else {
                return false;
            };
            (self.definition.controls.iter()).any(|alias| {
                alias.module == *module
                    && warning.refused.as_deref() == Some(alias.control.as_str())
                    && (refused.iter()).any(|own| own.refused.as_deref() == Some(&alias.name))
            })
        };
        if self.registry.builds_live() {
            let inner: Vec<_> = inner.into_iter().filter(|w| !passed_down(w)).collect();
            inner
                .into_iter()
                .chain(refused)
                .for_each(load_warnings::warn);
        }
        #[cfg(debug_assertions)]
        if let Some(controls) = &module.controls {
            let keys = self.config_keys();
            let modules = &module.graph.modules;
            replacing::check_rewrites(
                &self.definition,
                &self.name,
                keys,
                config,
                controls,
                modules,
            );
        }

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: Vec::<(String, Arc<dyn Any + Send + Sync>)>::new(),
            control_surface: Some(control_surface),
            sink: None,
        })
    }
}

pub(crate) struct DevelopmentModule {
    name: String,
    input_ports: Vec<&'static str>,
    output_ports: Vec<&'static str>,
    /// Per-external-input-port block buffer (fed by the parent graph).
    input_buffers: Vec<[f32; MAX_BLOCK]>,
    /// Per-external-output-port block buffer (read by the parent graph).
    output_buffers: Vec<[f32; MAX_BLOCK]>,
    outputs: Vec<ExternalOutputRoute>,
    graph: CompiledDevelopmentGraph,
    /// Exposed controls reaching declared inner controls.
    controls: Option<DevelopmentControls>,
    /// Its signal flow seen from outside (see `crate::invention::timelines`).
    enclosure: Arc<Enclosure>,
}

impl DevelopmentModule {
    fn new(
        name: &str,
        runtime: InventionRuntime,
        definition: &Invention,
    ) -> Result<(Self, Arc<dyn ControlSurface + Send + Sync>), Box<dyn std::error::Error>> {
        for input in &definition.inputs {
            let module = runtime
                .modules
                .get(&input.to)
                .ok_or_else(|| format!("Unknown input target module: {}", input.to))?;
            validate_input_port(module, &input.to_port)?;
        }

        for output in &definition.outputs {
            let module = runtime
                .modules
                .get(&output.from)
                .ok_or_else(|| format!("Unknown output source module: {}", output.from))?;
            validate_output_port(module, &output.from_port)?;
        }

        let input_ports = unique_port_names(definition.inputs.iter().map(|entry| &entry.name));
        let output_ports = unique_port_names(definition.outputs.iter().map(|entry| &entry.name));

        let mut route_indexes: HashMap<&str, usize> = HashMap::new();
        for (index, name) in input_ports.iter().enumerate() {
            route_indexes.insert(*name, index);
        }

        // Take ownership before building the index map to avoid a borrow
        // of the IndexMap that outlives the consumption point.
        let mut module_list: Vec<(String, GraphModule)> = runtime.modules.into_iter().collect();
        let module_indexes: HashMap<String, usize> = module_list
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (id.clone(), i))
            .collect();

        let surfaces = runtime.control_surfaces.lock().unwrap().clone();
        enclose(&surfaces, &mut module_list);
        let (controls, declared) =
            match DevelopmentControls::new(name, definition, &surfaces, &module_indexes)? {
                Some((controls, declared)) => (Some(controls), Some(declared)),
                None => (None, None),
            };
        // The inner modules grouped by their routes, as the main graph
        // groups its own: each loop runs sample by sample, the rest a block
        // at a time (see `process`).
        let routes = routes_of(&runtime.routing, &module_indexes_slice(&module_list));
        let entries: Vec<usize> = definition
            .inputs
            .iter()
            .filter_map(|input| module_indexes.get(input.to.as_str()).copied())
            .collect();
        let grouping = Grouping::of(module_list.len(), &routes, &entries);
        // What the surface orders the inner groups again from when a
        // payload retargets one of their modules (see `retarget`).
        let order = InnerOrder {
            ids: module_list.iter().map(|(id, _)| id.clone()).collect(),
            routes,
            group_of: grouping.group_of.clone(),
            groups: grouping.groups.len(),
        };
        // The surface keeps the internal directory alive once `runtime` is
        // consumed, so inner schedulers can still resolve schedules.
        let control_surface = Arc::new(DevelopmentControlSurface::new(
            name,
            definition,
            runtime.control_surfaces.clone(),
            declared,
            order,
        )?);

        let mut input_routes: Vec<Vec<ExternalInputRoute>> =
            (0..input_ports.len()).map(|_| Vec::new()).collect();
        for input in &definition.inputs {
            let index = *route_indexes
                .get(input.name.as_str())
                .ok_or_else(|| format!("Unknown development input: {}", input.name))?;
            let module_index = *module_indexes
                .get(input.to.as_str())
                .ok_or_else(|| format!("Unknown input target module: {}", input.to))?;
            let port_index = module_list
                .get(module_index)
                .and_then(|(_, m)| m.module().input_port_index(input.to_port.as_str()))
                .ok_or_else(|| {
                    format!(
                        "Unknown input port '{}' on module '{}'",
                        input.to_port, input.to
                    )
                })?;
            input_routes[index].push(ExternalInputRoute {
                module_index,
                port_index,
            });
        }

        let outputs = definition
            .outputs
            .iter()
            .enumerate()
            .map(|(index, output)| {
                let module_index = *module_indexes
                    .get(output.from.as_str())
                    .ok_or_else(|| format!("Unknown output source module: {}", output.from))?;
                let port_index = module_list
                    .get(module_index)
                    .and_then(|(_, m)| m.module().output_port_index(output.from_port.as_str()))
                    .ok_or_else(|| {
                        format!(
                            "Unknown output port '{}' on module '{}'",
                            output.from_port, output.from
                        )
                    })?;
                Ok(ExternalOutputRoute {
                    name: output_ports[index],
                    module_index,
                    port_index,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;

        let enclosure = Arc::new(enclosure_of(
            &module_list,
            &runtime.routing,
            &input_routes,
            &output_ports,
            &outputs,
        ));
        let mut graph = CompiledDevelopmentGraph::from_modules(
            module_list,
            &runtime.routing,
            &input_routes,
            grouping,
        )?;

        // Mark every sub-module input port that receives a signal (from an
        // external input or an internal route) as connected, so modules that
        // arbitrate signal-vs-control use the incoming value.
        for (module, routes) in graph.modules.iter_mut().zip(&graph.routes) {
            for route in routes {
                module.module_mut().set_input_connected(route.to_port, true);
            }
        }

        let input_count = input_ports.len();
        let output_count = output_ports.len();

        Ok((
            Self {
                name: name.to_string(),
                input_ports,
                output_ports,
                input_buffers: vec![[0.0; MAX_BLOCK]; input_count],
                output_buffers: vec![[0.0; MAX_BLOCK]; output_count],
                outputs,
                graph,
                controls,
                enclosure,
            },
            control_surface,
        ))
    }
}

impl Module for DevelopmentModule {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self, frames: usize) -> bool {
        self.process_graph(frames);
        true
    }

    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        self.controls.as_ref().map(DevelopmentControls::declared)
    }

    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match &self.controls {
            Some(controls) => controls.apply(&mut self.graph.modules, control, value),
            None => Err(Refusal::Unsupported),
        }
    }

    fn enclosure(&self) -> Option<&Arc<Enclosure>> {
        Some(&self.enclosure)
    }

    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        let Some(controls) = &self.controls else {
            return Err((Refusal::Unsupported, payload));
        };
        let graph = &mut self.graph;
        let modules = &mut graph.modules;
        let (order, written) = (&mut graph.group_order, &mut graph.written_within);
        apply_reordered(order, written, payload, |payload| {
            controls.apply_payload(modules, control, payload)
        })
    }

    /// Finishes the events of the inner modules an alias reached.
    fn finish_events(&mut self) {
        for module in &mut self.graph.modules {
            module.module_mut().finish_events();
        }
    }

    fn for_each_alias(
        &self,
        control: ControlIndex,
        f: &mut dyn FnMut(&ControlTable, &ControlCells, ControlIndex),
    ) {
        if let Some(controls) = &self.controls {
            controls.for_each_alias(&self.graph.modules, control, f);
        }
    }

    fn alias_value(&self, control: ControlIndex) -> Option<RtValue> {
        self.controls
            .as_ref()?
            .alias_value(&self.graph.modules, control)
    }

    /// Prepares every inner module, so a scheduler inside a development
    /// adopts its schedule here rather than on its first audio block.
    fn prepare_for_publication(&mut self) {
        for module in &mut self.graph.modules {
            module.module_mut().prepare_for_publication();
        }
    }

    fn inputs(&self) -> &[&str] {
        &self.input_ports
    }

    fn outputs(&self) -> &[&str] {
        &self.output_ports
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        &mut self.input_buffers[index]
    }

    fn output_block(&self, index: usize) -> &[f32] {
        &self.output_buffers[index]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = self
            .input_ports
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {}", port))?;
        self.input_buffers[index].fill(value);
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let index = self
            .outputs
            .iter()
            .position(|route| route.name == port)
            .ok_or_else(|| format!("Unknown output port: {}", port))?;
        Ok(self.output_buffers[index][0])
    }
}
