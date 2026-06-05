use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::graph::RoutingConnection;
use crate::invention::runtime::{
    validate_input_port, validate_output_port, ControlSurfaceInstance, InventionRuntime,
};
use crate::{
    ControlMeta, ControlSurface, ControlValue, Invention, Module, ModuleRegistry, MAX_BLOCK,
};
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
    port_index: usize,
}

struct ExternalOutputRoute {
    name: &'static str,
    module_index: usize,
    port_index: usize,
}

#[derive(Clone, Copy)]
struct CompiledConnection {
    from_module: usize,
    from_port: usize,
    to_port: usize,
}

struct CompiledDevelopmentGraph {
    modules: Vec<GraphModule>,
    input_routes: Vec<Vec<CompiledConnection>>,
    process_order: Vec<usize>,
    /// Output port count per internal module (parallel to module index).
    out_counts: Vec<usize>,
    /// Per-internal-module output block buffers, port-major with stride
    /// `MAX_BLOCK`: `out_bufs[module][port * MAX_BLOCK + frame]`.
    out_bufs: Vec<Vec<f32>>,
    /// True if the sub-graph has a feedback loop (forces the per-sample path).
    has_cycle: bool,
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
    /// Per-external-input-port block buffer (fed by the parent graph).
    input_buffers: Vec<[f32; MAX_BLOCK]>,
    /// Per-external-output-port block buffer (read by the parent graph).
    output_buffers: Vec<[f32; MAX_BLOCK]>,
    outputs: Vec<ExternalOutputRoute>,
    graph: CompiledDevelopmentGraph,
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

        // Take ownership before building the index map to avoid a borrow
        // of the IndexMap that outlives the consumption point.
        let module_list: Vec<(String, GraphModule)> = runtime.modules.into_iter().collect();
        let module_indexes: HashMap<String, usize> = module_list
            .iter()
            .enumerate()
            .map(|(i, (id, _))| (id.clone(), i))
            .collect();

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

        let mut graph = CompiledDevelopmentGraph::from_modules(module_list, &runtime.routing)?;

        // Mark every sub-module input port that receives a signal (from an
        // external input or an internal route) as connected, so modules that
        // arbitrate signal-vs-control use the incoming value.
        for routes in &input_routes {
            for route in routes {
                if let Some(module) = graph.modules.get_mut(route.module_index) {
                    module
                        .module_mut()
                        .set_input_connected(route.port_index, true);
                }
            }
        }
        for to_module in 0..graph.input_routes.len() {
            let count = graph.input_routes[to_module].len();
            for c in 0..count {
                let to_port = graph.input_routes[to_module][c].to_port;
                if let Some(module) = graph.modules.get_mut(to_module) {
                    module.module_mut().set_input_connected(to_port, true);
                }
            }
        }

        let input_count = input_ports.len();
        let output_count = output_ports.len();

        Ok((
            Self {
                name: name.to_string(),
                input_ports,
                output_ports,
                input_routes,
                input_buffers: vec![[0.0; MAX_BLOCK]; input_count],
                output_buffers: vec![[0.0; MAX_BLOCK]; output_count],
                outputs,
                graph,
            },
            control_surface,
        ))
    }

    /// Processes the internal sub-graph a whole block at a time (acyclic case).
    /// Each internal module's `process(frames)` is called once per block,
    /// restoring block amortization through nested voices.
    fn process_block(&mut self, frames: usize) {
        // Feed external input ports (overwrite, as the prior dev module did).
        for index in 0..self.input_routes.len() {
            for r in 0..self.input_routes[index].len() {
                let mi = self.input_routes[index][r].module_index;
                let pp = self.input_routes[index][r].port_index;
                let dst = self.graph.modules[mi].module_mut().input_block_mut(pp);
                dst[..frames].copy_from_slice(&self.input_buffers[index][..frames]);
            }
        }

        self.graph.current_sample += frames as u64;

        // Process internal modules full-block in topological order.
        for oi in 0..self.graph.process_order.len() {
            let m = self.graph.process_order[oi];
            for c in 0..self.graph.input_routes[m].len() {
                let conn = self.graph.input_routes[m][c];
                let base = conn.from_port * MAX_BLOCK;
                let dst = self.graph.modules[m].module_mut().input_block_mut(conn.to_port);
                dst[..frames]
                    .copy_from_slice(&self.graph.out_bufs[conn.from_module][base..base + frames]);
            }

            self.graph.modules[m].module_mut().process(frames);

            let n_out = self.graph.out_counts[m];
            for p in 0..n_out {
                let base = p * MAX_BLOCK;
                let src = self.graph.modules[m].module().output_block(p);
                self.graph.out_bufs[m][base..base + frames].copy_from_slice(&src[..frames]);
            }
        }

        // Cache external output ports.
        for oi in 0..self.outputs.len() {
            let from = self.outputs[oi].module_index;
            let base = self.outputs[oi].port_index * MAX_BLOCK;
            self.output_buffers[oi][..frames]
                .copy_from_slice(&self.graph.out_bufs[from][base..base + frames]);
        }
    }

    /// Processes the internal sub-graph sample-by-sample. Used only when the
    /// sub-graph contains a feedback loop, where back-edges must observe a
    /// one-sample delay (the pre-block behavior).
    fn process_per_sample(&mut self, frames: usize) {
        for i in 0..frames {
            for (index, routes) in self.input_routes.iter().enumerate() {
                let value = self.input_buffers[index][i];
                for route in routes {
                    if let Some(module) = self.graph.modules.get_mut(route.module_index) {
                        module.module_mut().input_block_mut(route.port_index)[0] = value;
                    }
                }
            }

            self.graph.current_sample += 1;

            for oi in 0..self.graph.process_order.len() {
                let module_index = self.graph.process_order[oi];
                let count = self.graph.input_routes[module_index].len();
                for c in 0..count {
                    let conn = self.graph.input_routes[module_index][c];
                    let input_value = self
                        .graph
                        .modules
                        .get(conn.from_module)
                        .map(|module| module.module().output_block(conn.from_port)[0])
                        .unwrap_or(0.0);
                    if let Some(module) = self.graph.modules.get_mut(module_index) {
                        module.module_mut().input_block_mut(conn.to_port)[0] = input_value;
                    }
                }

                if let Some(module) = self.graph.modules.get_mut(module_index) {
                    module.module_mut().process(1);
                }
            }

            for (oi, output) in self.outputs.iter().enumerate() {
                let value = self
                    .graph
                    .modules
                    .get(output.module_index)
                    .map(|module| module.module().output_block(output.port_index)[0])
                    .unwrap_or(0.0);
                self.output_buffers[oi][i] = value;
            }
        }
    }
}

impl Module for DevelopmentModule {
    fn name(&self) -> &str {
        &self.name
    }

    fn process(&mut self, frames: usize) -> bool {
        if self.graph.has_cycle {
            self.process_per_sample(frames);
        } else {
            self.process_block(frames);
        }
        true
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

fn module_indexes_slice(modules: &[(String, GraphModule)]) -> HashMap<&str, usize> {
    modules
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (id.as_str(), index))
        .collect()
}

impl CompiledDevelopmentGraph {
    fn from_modules(
        module_list: Vec<(String, GraphModule)>,
        routing: &[RoutingConnection],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let indexes = module_indexes_slice(&module_list);
        let (process_order, has_cycle) = compute_process_order_slice(&module_list, routing);
        let mut input_routes: Vec<Vec<CompiledConnection>> =
            (0..module_list.len()).map(|_| Vec::new()).collect();

        for conn in routing {
            let from_module = *indexes
                .get(conn.from_module.as_str())
                .ok_or_else(|| format!("Unknown source module: {}", conn.from_module))?;
            let to_module = *indexes
                .get(conn.to_module.as_str())
                .ok_or_else(|| format!("Unknown destination module: {}", conn.to_module))?;

            let from_port = module_list
                .get(from_module)
                .and_then(|(_, m)| m.module().output_port_index(conn.from_port.as_str()))
                .ok_or_else(|| {
                    format!(
                        "Unknown output port '{}' on module '{}'",
                        conn.from_port, conn.from_module
                    )
                })?;
            let to_port = module_list
                .get(to_module)
                .and_then(|(_, m)| m.module().input_port_index(conn.to_port.as_str()))
                .ok_or_else(|| {
                    format!(
                        "Unknown input port '{}' on module '{}'",
                        conn.to_port, conn.to_module
                    )
                })?;

            input_routes[to_module].push(CompiledConnection {
                from_module,
                from_port,
                to_port,
            });
        }

        let modules: Vec<GraphModule> = module_list.into_iter().map(|(_, m)| m).collect();

        // Per-internal-module output block buffers.
        let out_counts: Vec<usize> = modules.iter().map(|m| m.module().outputs().len()).collect();
        let out_bufs: Vec<Vec<f32>> = out_counts.iter().map(|&c| vec![0.0; c * MAX_BLOCK]).collect();

        Ok(Self {
            modules,
            input_routes,
            process_order,
            out_counts,
            out_bufs,
            has_cycle,
            current_sample: 0,
        })
    }
}

/// Returns `(process_order, has_cycle)`. `has_cycle` is true if the sub-graph
/// contains a back-edge (a feedback loop), which forces the slower
/// sample-by-sample processing path.
fn compute_process_order_slice(
    modules: &[(String, GraphModule)],
    routing: &[RoutingConnection],
) -> (Vec<usize>, bool) {
    let indexes = module_indexes_slice(modules);
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
    let mut has_cycle = false;

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
                        // Back-edge: a feedback loop. Preserve the existing
                        // one-sample-delay behavior via the per-sample path.
                        has_cycle = true;
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
    (order, has_cycle)
}
