use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::invention::graph::{RoutingConnection, SignalGraph};
use crate::invention::runtime::{
    validate_input_port, validate_output_port, ControlSurfaceInstance, InventionRuntime,
};
use crate::{ControlMeta, ControlSurface, ControlValue, Invention, Module, ModuleRegistry};
use indexmap::IndexMap;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
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
            module: Arc::new(Mutex::new(module)),
            handles: Vec::<(String, Arc<dyn Any + Send + Sync>)>::new(),
            control_surface: Some(control_surface),
            sink: None,
        })
    }
}

struct ExternalInputRoute {
    module_id: String,
    port: String,
}

struct ExternalOutputRoute {
    name: &'static str,
    module_id: String,
    port: String,
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
    graph: SignalGraph,
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

        let input_ports: Vec<&'static str> = definition
            .inputs
            .iter()
            .map(|entry| leak_name(&entry.name))
            .collect();
        let output_ports: Vec<&'static str> = definition
            .outputs
            .iter()
            .map(|entry| leak_name(&entry.name))
            .collect();

        let mut route_indexes: HashMap<&str, usize> = HashMap::new();
        for (index, name) in input_ports.iter().enumerate() {
            route_indexes.insert(*name, index);
        }

        let mut input_routes: Vec<Vec<ExternalInputRoute>> =
            (0..input_ports.len()).map(|_| Vec::new()).collect();
        for input in &definition.inputs {
            let index = *route_indexes
                .get(input.name.as_str())
                .ok_or_else(|| format!("Unknown development input: {}", input.name))?;
            input_routes[index].push(ExternalInputRoute {
                module_id: input.to.clone(),
                port: input.to_port.clone(),
            });
        }

        let outputs = definition
            .outputs
            .iter()
            .enumerate()
            .map(|(index, output)| ExternalOutputRoute {
                name: output_ports[index],
                module_id: output.from.clone(),
                port: output.from_port.clone(),
            })
            .collect();

        let mut input_map: HashMap<String, Vec<RoutingConnection>> = HashMap::new();
        for conn in &runtime.routing {
            input_map
                .entry(conn.to_module.clone())
                .or_default()
                .push(conn.clone());
        }

        let (_, command_rx) = mpsc::channel();
        let graph = SignalGraph {
            modules: runtime.modules,
            sinks: runtime.sinks,
            input_map,
            current_sample: 0,
            command_rx,
            process_order: Vec::new(),
            topo_dirty: true,
        };

        Ok((
            Self {
                name: name.to_string(),
                input_ports,
                output_ports,
                input_routes,
                input_values: vec![0.0; definition.inputs.len()],
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
                if let Some(module) = self.graph.modules.get(&route.module_id) {
                    let _ = module.lock().unwrap().set_input(&route.port, value);
                }
            }
        }
    }
}

impl Module for DevelopmentModule {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self) -> bool {
        self.graph.ensure_process_order();

        for module in self.graph.modules.values() {
            module.lock().unwrap().reset_inputs();
        }
        self.graph.current_sample += 1;
        self.apply_external_inputs();

        for i in 0..self.graph.process_order.len() {
            let module_id = &self.graph.process_order[i];

            if let Some(connections) = self.graph.input_map.get(module_id.as_str()) {
                for conn in connections {
                    let input_value = self
                        .graph
                        .modules
                        .get(&conn.from_module)
                        .map(|m| m.lock().unwrap().get_output(&conn.from_port).unwrap_or(0.0))
                        .unwrap_or(0.0);
                    if let Some(module) = self.graph.modules.get(module_id.as_str()) {
                        let _ = module.lock().unwrap().set_input(&conn.to_port, input_value);
                    }
                }
            }

            if let Some(module) = self.graph.modules.get(module_id.as_str()) {
                let mut module = module.lock().unwrap();
                module.process();
                module.mark_processed(self.graph.current_sample);
            }
        }

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
        self.graph
            .modules
            .get(&output.module_id)
            .ok_or_else(|| format!("Unknown output module: {}", output.module_id))?
            .lock()
            .unwrap()
            .get_output(&output.port)
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
