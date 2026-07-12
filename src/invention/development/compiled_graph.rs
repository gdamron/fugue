use super::*;

pub(super) struct ExternalInputRoute {
    pub(super) module_index: usize,
    pub(super) port_index: usize,
}

pub(super) struct ExternalOutputRoute {
    pub(super) name: &'static str,
    pub(super) module_index: usize,
    pub(super) port_index: usize,
}

#[derive(Clone, Copy)]
pub(super) struct CompiledConnection {
    pub(super) from_module: usize,
    pub(super) from_port: usize,
    pub(super) to_port: usize,
}

pub(super) struct CompiledDevelopmentGraph {
    pub(super) modules: Vec<GraphModule>,
    pub(super) input_routes: Vec<Vec<CompiledConnection>>,
    pub(super) process_order: Vec<usize>,
    /// Output port count per internal module (parallel to module index).
    pub(super) out_counts: Vec<usize>,
    /// Per-internal-module output block buffers, port-major with stride
    /// `MAX_BLOCK`: `out_bufs[module][port * MAX_BLOCK + frame]`.
    pub(super) out_bufs: Vec<Vec<f32>>,
    /// True if the sub-graph has a feedback loop (forces the per-sample path).
    pub(super) has_cycle: bool,
    pub(super) current_sample: u64,
}

pub(super) fn leak_name(name: &str) -> &'static str {
    Box::leak(name.to_string().into_boxed_str())
}

pub(super) fn unique_port_names<'a>(names: impl Iterator<Item = &'a String>) -> Vec<&'static str> {
    let mut ports = Vec::new();
    let mut seen = HashSet::new();

    for name in names {
        if seen.insert(name.as_str()) {
            ports.push(leak_name(name));
        }
    }

    ports
}

pub(super) fn module_indexes_slice(modules: &[(String, GraphModule)]) -> HashMap<&str, usize> {
    modules
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (id.as_str(), index))
        .collect()
}

impl CompiledDevelopmentGraph {
    pub(super) fn from_modules(
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
        let out_bufs: Vec<Vec<f32>> = out_counts
            .iter()
            .map(|&c| vec![0.0; c * MAX_BLOCK])
            .collect();

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
pub(super) fn compute_process_order_slice(
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
