use super::*;
use crate::invention::timelines::{self, Flow, Link};

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
        let routes = routes_of(routing, &indexes);
        let targets = targets_of(module_list.iter().map(|(_, m)| m), &indexes);
        let (process_order, has_cycle) = process_order(module_list.len(), &routes, &targets);
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

/// Each route as `(from, to)` module indexes, in routing order; routes
/// naming no inner module are left out.
pub(super) fn routes_of(
    routing: &[RoutingConnection],
    indexes: &HashMap<&str, usize>,
) -> Vec<(usize, usize)> {
    routing
        .iter()
        .filter_map(|conn| {
            let from = *indexes.get(conn.from_module.as_str())?;
            let to = *indexes.get(conn.to_module.as_str())?;
            Some((from, to))
        })
        .collect()
}

/// Each module's control targets, by index; ids naming no inner module are
/// left out.
fn targets_of<'a>(
    modules: impl Iterator<Item = &'a GraphModule>,
    indexes: &HashMap<&str, usize>,
) -> Vec<Vec<usize>> {
    modules
        .map(|module| {
            let targets = module.module().control_targets();
            targets
                .iter()
                .filter_map(|id| indexes.get(id.as_str()).copied())
                .collect()
        })
        .collect()
}

/// The order of `n` modules joined by `routes` (`(from, to)` pairs, in
/// routing order) and ordering edges to each one's `targets`, as
/// `(process_order, has_cycle)`. `has_cycle` is true if the sub-graph
/// contains a back-edge (a feedback loop), which forces the slower
/// sample-by-sample processing path.
///
/// As at the top level (see `crate::invention::graph::compile`), a module
/// runs before the modules whose controls it writes (see
/// [`crate::Module::control_targets`]), so an inner scheduler's write lands
/// in the block it falls in. Unlike there, such an ordering edge that would
/// close a loop is left out rather than folded into a feedback group: a
/// development has one processing path, and a loop would move all of it to
/// the per-sample path. So only routing decides `has_cycle`, and a
/// scheduler writing a module upstream of it (its own clock, say) lands its
/// write a block later, as before.
pub(super) fn process_order(
    n: usize,
    routes: &[(usize, usize)],
    targets: &[Vec<usize>],
) -> (Vec<usize>, bool) {
    let mut downstream: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
    for &(from, to) in routes {
        downstream[from].push(to);
    }

    // Ordering edges join after the routes, so a sub-graph without them
    // orders exactly as by routing alone.
    for (from, targets) in targets.iter().enumerate() {
        for &to in targets {
            if !reaches(&downstream, to, from) {
                downstream[from].push(to);
            }
        }
    }

    let mut state = vec![0_u8; n];
    let mut order = Vec::with_capacity(n);
    let mut has_cycle = false;

    for start in 0..n {
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
                        // Every loop is made of routes: no ordering edge
                        // closing one was added.
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

/// Whether `to` is reachable from `from` along `downstream` (a module
/// reaches itself).
fn reaches(downstream: &[Vec<usize>], from: usize, to: usize) -> bool {
    let mut seen = vec![false; downstream.len()];
    let mut stack = vec![from];
    seen[from] = true;
    while let Some(node) = stack.pop() {
        if node == to {
            return true;
        }
        for &next in &downstream[node] {
            if !seen[next] {
                seen[next] = true;
                stack.push(next);
            }
        }
    }
    false
}

/// A development's signal flow seen from outside: what reaches each output
/// port (by index) from its inner modules and input ports.
pub(super) fn enclosure_of(
    modules: &[(String, GraphModule)],
    routing: &[RoutingConnection],
    input_routes: &[Vec<ExternalInputRoute>],
    output_ports: &[&'static str],
    outputs: &[ExternalOutputRoute],
) -> Enclosure {
    let flows: Vec<Flow> = modules.iter().map(|(_, m)| Flow::of(m.module())).collect();
    let index = |id: &str| modules.iter().position(|(other, _)| other == id);
    let links: Vec<Link> = routing
        .iter()
        .filter_map(|conn| {
            let from = index(&conn.from_module)?;
            let to = index(&conn.to_module)?;
            Some(Link {
                from,
                from_port: modules[from]
                    .1
                    .module()
                    .output_port_index(&conn.from_port)?,
                to,
                to_port: modules[to].1.module().input_port_index(&conn.to_port)?,
            })
        })
        .collect();
    let boundary: Vec<_> = input_routes
        .iter()
        .enumerate()
        .flat_map(|(input, routes)| {
            routes
                .iter()
                .map(move |route| (route.module_index, route.port_index, input))
        })
        .collect();
    let sources = output_ports.iter().map(|name| {
        let route = outputs.iter().find(|route| route.name == *name)?;
        Some((route.module_index, route.port_index))
    });
    timelines::enclosure(&flows, &links, &boundary, sources)
}
