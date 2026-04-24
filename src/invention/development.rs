use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::graph::RoutingConnection;
use crate::invention::runtime::{
    validate_input_port, validate_output_port, ControlSurfaceInstance, InventionRuntime,
};
use crate::{ControlMeta, ControlSurface, ControlValue, Invention, Module, ModuleRegistry};
use indexmap::IndexMap;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::builder::InventionBuilder;

pub(crate) struct DevelopmentFactory {
    pub(crate) name: String,
    pub(crate) definition: Invention,
    pub(crate) registry: ModuleRegistry,
    pub(crate) registered: Arc<Mutex<HashSet<String>>>,
}

impl ModuleFactory for DevelopmentFactory {
    fn type_id(&self) -> &'static str {
        "__development__"
    }

    fn build(
        &self,
        sample_rate: u32,
        _config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let builder = InventionBuilder::with_registry_and_registered(
            sample_rate,
            self.registry.clone(),
            self.registered.clone(),
        );
        let (runtime, _handles) = builder.build(self.definition.clone())?;
        let (module, control_surface) =
            DevelopmentModule::new(&self.name, runtime, &self.definition)?;

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: Vec::<(String, Arc<dyn Any + Send + Sync>)>::new(),
            control_surface: Some(control_surface),
            sink: None,
        })
    }
}

struct ExternalInputRoute {
    module_index: usize,
    port: String,
}

struct ExternalOutputRoute {
    name: &'static str,
    module_index: usize,
    port: String,
    value: f32,
}

struct CompiledConnection {
    from_module: usize,
    from_port: String,
    to_port: String,
}

struct CompiledDevelopmentGraph {
    modules: Vec<GraphModule>,
    input_routes: Vec<Vec<CompiledConnection>>,
    process_order: Vec<usize>,
    current_sample: u64,
}

struct AliasedControl {
    meta: ControlMeta,
    module_id: String,
    key: String,
}

struct DevelopmentControlSurface {
    controls: Vec<AliasedControl>,
    surfaces: IndexMap<String, ControlSurfaceInstance>,
}

impl DevelopmentControlSurface {
    fn new(
        definition: &Invention,
        surfaces: &IndexMap<String, ControlSurfaceInstance>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut controls = Vec::with_capacity(definition.controls.len());

        for control in &definition.controls {
            let surface = surfaces
                .get(&control.module)
                .ok_or_else(|| format!("Unknown control module: {}", control.module))?;
            let source = surface
                .controls()
                .into_iter()
                .find(|meta| meta.key == control.control)
                .ok_or_else(|| {
                    format!(
                        "Unknown control '{}' on module '{}'",
                        control.control, control.module
                    )
                })?;

            controls.push(AliasedControl {
                meta: ControlMeta {
                    key: control.key.clone(),
                    description: source.description,
                    default: source.default,
                    kind: source.kind,
                },
                module_id: control.module.clone(),
                key: control.control.clone(),
            });
        }

        Ok(Self {
            controls,
            surfaces: surfaces.clone(),
        })
    }

    fn lookup(&self, key: &str) -> Result<(&AliasedControl, &ControlSurfaceInstance), String> {
        let control = self
            .controls
            .iter()
            .find(|entry| entry.meta.key == key)
            .ok_or_else(|| format!("Unknown control: {}", key))?;
        let surface = self
            .surfaces
            .get(&control.module_id)
            .ok_or_else(|| format!("Unknown control module: {}", control.module_id))?;
        Ok((control, surface))
    }
}

impl ControlSurface for DevelopmentControlSurface {
    fn controls(&self) -> Vec<ControlMeta> {
        self.controls
            .iter()
            .map(|entry| entry.meta.clone())
            .collect()
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        let (control, surface) = self.lookup(key)?;
        surface.get_control(&control.key)
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let (control, surface) = self.lookup(key)?;
        surface.set_control(&control.key, value)
    }
}

pub(crate) struct DevelopmentModule {
    name: String,
    input_ports: Vec<&'static str>,
    output_ports: Vec<&'static str>,
    input_routes: Vec<Vec<ExternalInputRoute>>,
    input_values: Vec<f32>,
    outputs: Vec<ExternalOutputRoute>,
    graph: CompiledDevelopmentGraph,
    last_processed_sample: u64,
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

        let control_surface = Arc::new(DevelopmentControlSurface::new(
            definition,
            &runtime.control_surfaces,
        )?);

        let input_ports = unique_port_names(definition.inputs.iter().map(|entry| &entry.name));
        let output_ports = unique_port_names(definition.outputs.iter().map(|entry| &entry.name));

        let mut route_indexes: HashMap<&str, usize> = HashMap::new();
        for (index, name) in input_ports.iter().enumerate() {
            route_indexes.insert(*name, index);
        }

        let module_indexes = module_indexes(&runtime.modules);

        let mut input_routes: Vec<Vec<ExternalInputRoute>> =
            (0..input_ports.len()).map(|_| Vec::new()).collect();
        for input in &definition.inputs {
            let index = *route_indexes
                .get(input.name.as_str())
                .ok_or_else(|| format!("Unknown development input: {}", input.name))?;
            let module_index = *module_indexes
                .get(input.to.as_str())
                .ok_or_else(|| format!("Unknown input target module: {}", input.to))?;
            input_routes[index].push(ExternalInputRoute {
                module_index,
                port: input.to_port.clone(),
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
                Ok(ExternalOutputRoute {
                    name: output_ports[index],
                    module_index,
                    port: output.from_port.clone(),
                    value: 0.0,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;

        let graph = CompiledDevelopmentGraph::new(runtime.modules, &runtime.routing)?;

        Ok((
            Self {
                name: name.to_string(),
                input_ports,
                output_ports,
                input_routes,
                input_values: vec![0.0; route_indexes.len()],
                outputs,
                graph,
                last_processed_sample: 0,
            },
            control_surface,
        ))
    }

    fn apply_external_inputs(&mut self) {
        for (index, routes) in self.input_routes.iter().enumerate() {
            let value = self.input_values[index];
            for route in routes {
                if let Some(module) = self.graph.modules.get_mut(route.module_index) {
                    let _ = module.module_mut().set_input(&route.port, value);
                }
            }
        }
    }

    fn cache_outputs(&mut self) {
        for output in &mut self.outputs {
            output.value = self
                .graph
                .modules
                .get(output.module_index)
                .map(|module| module.module().get_output(&output.port).unwrap_or(0.0))
                .unwrap_or(0.0);
        }
    }
}

impl Module for DevelopmentModule {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self) -> bool {
        for module in &mut self.graph.modules {
            module.module_mut().reset_inputs();
        }
        self.graph.current_sample += 1;
        self.apply_external_inputs();

        for &module_index in &self.graph.process_order {
            if let Some(connections) = self.graph.input_routes.get(module_index) {
                for conn in connections {
                    let input_value = self
                        .graph
                        .modules
                        .get(conn.from_module)
                        .map(|module| module.module().get_output(&conn.from_port).unwrap_or(0.0))
                        .unwrap_or(0.0);
                    if let Some(module) = self.graph.modules.get_mut(module_index) {
                        let _ = module.module_mut().set_input(&conn.to_port, input_value);
                    }
                }
            }

            if let Some(module) = self.graph.modules.get_mut(module_index) {
                let module = module.module_mut();
                module.process();
                module.mark_processed(self.graph.current_sample);
            }
        }

        self.cache_outputs();
        self.last_processed_sample = self.graph.current_sample;
        true
    }

    fn inputs(&self) -> &[&str] {
        &self.input_ports
    }

    fn outputs(&self) -> &[&str] {
        &self.output_ports
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        let index = self
            .input_ports
            .iter()
            .position(|name| *name == port)
            .ok_or_else(|| format!("Unknown input port: {}", port))?;
        self.input_values[index] = value;
        Ok(())
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        let output = self
            .outputs
            .iter()
            .find(|route| route.name == port)
            .ok_or_else(|| format!("Unknown output port: {}", port))?;
        Ok(output.value)
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }
}

fn leak_name(name: &str) -> &'static str {
    Box::leak(name.to_string().into_boxed_str())
}

fn unique_port_names<'a>(names: impl Iterator<Item = &'a String>) -> Vec<&'static str> {
    let mut ports = Vec::new();
    let mut seen = HashSet::new();

    for name in names {
        if seen.insert(name.as_str()) {
            ports.push(leak_name(name));
        }
    }

    ports
}

fn module_indexes(modules: &IndexMap<String, GraphModule>) -> HashMap<&str, usize> {
    modules
        .keys()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect()
}

impl CompiledDevelopmentGraph {
    fn new(
        modules: IndexMap<String, GraphModule>,
        routing: &[RoutingConnection],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let indexes = module_indexes(&modules);
        let process_order = compute_process_order(&modules, routing);
        let mut input_routes: Vec<Vec<CompiledConnection>> =
            (0..modules.len()).map(|_| Vec::new()).collect();

        for conn in routing {
            let from_module = *indexes
                .get(conn.from_module.as_str())
                .ok_or_else(|| format!("Unknown source module: {}", conn.from_module))?;
            let to_module = *indexes
                .get(conn.to_module.as_str())
                .ok_or_else(|| format!("Unknown destination module: {}", conn.to_module))?;
            input_routes[to_module].push(CompiledConnection {
                from_module,
                from_port: conn.from_port.clone(),
                to_port: conn.to_port.clone(),
            });
        }

        Ok(Self {
            modules: modules.into_values().collect(),
            input_routes,
            process_order,
            current_sample: 0,
        })
    }
}

fn compute_process_order(
    modules: &IndexMap<String, GraphModule>,
    routing: &[RoutingConnection],
) -> Vec<usize> {
    let indexes = module_indexes(modules);
    let mut downstream: Vec<Vec<usize>> = (0..modules.len()).map(|_| Vec::new()).collect();

    for conn in routing {
        let Some(&from_index) = indexes.get(conn.from_module.as_str()) else {
            continue;
        };
        let Some(&to_index) = indexes.get(conn.to_module.as_str()) else {
            continue;
        };
        downstream[from_index].push(to_index);
    }

    let mut state = vec![0_u8; modules.len()];
    let mut order = Vec::with_capacity(modules.len());

    for start in 0..modules.len() {
        if state[start] != 0 {
            continue;
        }

        let mut stack = vec![(start, 0_usize)];
        state[start] = 1;

        while let Some((node, next_index)) = stack.last_mut() {
            if *next_index < downstream[*node].len() {
                let next = downstream[*node][*next_index];
                *next_index += 1;
                match state[next] {
                    0 => {
                        state[next] = 1;
                        stack.push((next, 0));
                    }
                    1 => {
                        // Back-edge: preserve the existing graph behavior by treating
                        // the connection as a one-sample delay point.
                    }
                    _ => {}
                }
            } else {
                let (finished, _) = stack.pop().unwrap();
                state[finished] = 2;
                order.push(finished);
            }
        }
    }

    order.reverse();
    order
}
