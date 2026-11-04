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
use std::sync::{Arc, Mutex};

use super::builder::InventionBuilder;
use super::reload::LoadedDevelopments;
use super::timelines::Enclosure;

mod compiled_graph;
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
mod replacing;
mod retarget;
#[cfg(test)]
mod sample_player_tests;
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
        let builder = InventionBuilder::with_registry_and_registered(
            sample_rate,
            self.registry.clone(),
            self.registered.clone(),
            self.loaded.clone(),
        );
        let (runtime, _handles) = builder.build(self.definition.clone())?;
        let (module, control_surface) =
            DevelopmentModule::new(&self.name, runtime, &self.definition)?;
        apply_development_config(&self.name, config, control_surface.as_ref())?;
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

/// Applies a development instance's config as initial values for its exposed
/// controls. A development has no other config surface, so every key must
/// name an exposed control and every value must be a scalar; this is what
/// lets a document whose control changes were written into a development
/// instance's config rebuild with the same values on a cold load. A key
/// naming an event (one an alias fires) holds no state to restore: it is
/// read and ignored, as a plain module's build ignores one, and a reload
/// ignores it too (see `plan_reload`).
fn apply_development_config(
    name: &str,
    config: &serde_json::Value,
    surface: &(dyn ControlSurface + Send + Sync),
) -> Result<(), Box<dyn std::error::Error>> {
    let map = match config {
        serde_json::Value::Null => return Ok(()),
        serde_json::Value::Object(map) => map,
        _ => return Err(format!("Development '{}' config must be an object", name).into()),
    };
    let mut pending = Vec::with_capacity(map.len());
    for (key, value) in map {
        if surface
            .declaration(key)
            .is_some_and(|found| found.decl.event)
        {
            continue;
        }
        let value = crate::traits::exact_number::json_scalar(value).ok_or_else(|| {
            format!(
                "Development '{}' config key '{}' must be a number, bool, or string",
                name, key
            )
        })?;
        // Number text an f32 misreads stays text only for a whole-number
        // control; any other number control is handed a number.
        pending.push((key, surface.coerce_value(key, value)));
    }
    // A payload (a melody's whole scale) applies before the scalars, which
    // may write within it.
    pending.sort_by_key(|(key, _)| {
        !surface
            .declaration(key)
            .is_some_and(|found| found.decl.kind == crate::control_request::DeclKind::Payload)
    });
    // Keys are applied in passes, a failed one again after the rest, since
    // one control can size what another reaches (a count and a degree
    // behind it). A key still failing whose control is listed but cannot be
    // read is one a count hides now: it was written before the count shrank,
    // and is left out. Any other failure fails the build.
    loop {
        let before = pending.len();
        let mut failed = Vec::new();
        for (key, value) in pending {
            if let Err(error) = surface.set_control(key, value.clone()) {
                failed.push((key, value, error));
            }
        }
        if failed.is_empty() {
            return Ok(());
        }
        if failed.len() == before {
            let listed = surface.controls();
            for (key, _, error) in failed {
                let hidden =
                    listed.iter().any(|meta| &meta.key == key) && surface.get_control(key).is_err();
                if !hidden {
                    return Err(format!("Development '{}' config: {}", name, error).into());
                }
            }
            return Ok(());
        }
        pending = failed
            .into_iter()
            .map(|(key, value, _)| (key, value))
            .collect();
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
            match DevelopmentControls::new(definition, &surfaces, &module_indexes)? {
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
        let modules = &mut self.graph.modules;
        apply_reordered(&mut self.graph.group_order, payload, |payload| {
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
