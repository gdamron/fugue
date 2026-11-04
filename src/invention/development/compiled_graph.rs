use super::*;
use crate::invention::graph::scc::{
    is_back_edge, positions, process_groups, topological_order, topological_order_from,
};
use crate::invention::graph::ProcessGroup;
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

/// Where a route into an inner module reads from.
#[derive(Clone, Copy)]
pub(super) enum Source {
    /// Output port `port` of inner module `module`.
    Module { module: usize, port: usize },
    /// The development's own input port, by index.
    Input(usize),
}

/// A route into an inner module's input port, as the main graph sums its
/// routes (see `crate::invention::graph::process`).
#[derive(Clone, Copy)]
pub(super) struct InnerRoute {
    pub(super) source: Source,
    pub(super) to_port: usize,
    /// A loop's back-edge: the destination reads the source's previous
    /// sample (see [`is_back_edge`]).
    pub(super) delayed: bool,
    /// The first route into `to_port`: it writes `0.0 + value`, exactly
    /// the main graph's zeroing then summing, and later routes add to it.
    pub(super) first: bool,
}

pub(super) struct CompiledDevelopmentGraph {
    pub(super) modules: Vec<GraphModule>,
    /// The routes into each inner module: the development's inputs first,
    /// then inner routes in routing order.
    pub(super) routes: Vec<Vec<InnerRoute>>,
    /// The inner modules grouped as the main graph groups its own (see
    /// [`Grouping`]).
    pub(super) groups: Vec<ProcessGroup>,
    /// The groups in the order they run, every group once: a whole block
    /// at a time for one acyclic module, sample by sample for a feedback
    /// group. A retargeting payload replaces it (see `retarget`).
    pub(super) group_order: Vec<usize>,
    /// Output port count per internal module (parallel to module index).
    pub(super) out_counts: Vec<usize>,
    /// Per-internal-module output block buffers, port-major with stride
    /// `MAX_BLOCK`: `out_bufs[module][port * MAX_BLOCK + frame]`.
    pub(super) out_bufs: Vec<Vec<f32>>,
    /// Each output port's last sample of the previous call, which a
    /// back-edge reads at frame 0.
    pub(super) out_prev: Vec<Vec<f32>>,
    pub(super) current_sample: u64,
}

/// A development's inner modules grouped by their routes alone, with the
/// main graph's grouping (see [`process_groups`]): each loop a feedback
/// group, run sample by sample, every other module a group of its own, run
/// a block at a time. What each module writes orders the groups (see
/// [`process_order`]) but never joins or splits one, so a payload that
/// retargets an inner scheduler leaves the groups, and which route of a
/// loop is delayed, as they are.
pub(super) struct Grouping {
    pub(super) groups: Vec<ProcessGroup>,
    pub(super) group_of: Vec<usize>,
    /// Each module's position in the per-sample order of the routes.
    pos: Vec<usize>,
}

impl Grouping {
    /// The grouping of `n` modules joined by `routes` (`(from, to)` pairs,
    /// in routing order), entered at `entries`: the modules the
    /// development's inputs feed, in input order. The walk that orders a
    /// loop's members starts there, as the main graph's does from the
    /// modules upstream of the development. A development is built without
    /// its enclosing graph, so this is the delay the same wiring gets flat
    /// when the enclosing graph's depth-first walk reaches the inputs'
    /// targets in the inputs' declared order, ahead of the rest: one that
    /// reaches them otherwise can delay the other route of a loop entered
    /// at two inputs.
    pub(super) fn of(n: usize, routes: &[(usize, usize)], entries: &[usize]) -> Self {
        let mut downstream: Vec<Vec<usize>> = (0..n).map(|_| Vec::new()).collect();
        for &(from, to) in routes {
            downstream[from].push(to);
        }
        let pos = positions(&topological_order_from(&downstream, entries));
        let (groups, group_of) = process_groups(&downstream, &pos);
        Self {
            groups,
            group_of,
            pos,
        }
    }
}

/// The groups in the order `order` (every module once) first reaches a
/// member of each. Run so, a group follows every module and group it reads
/// or is written by: in a depth-first order, a component's first module
/// comes before the first module of every component downstream of it.
/// Control thread: allocates.
pub(super) fn group_sequence(order: &[usize], group_of: &[usize], groups: usize) -> Vec<usize> {
    let mut seen = vec![false; groups];
    let mut sequence = Vec::with_capacity(groups);
    for &module in order {
        let group = group_of[module];
        if !seen[group] {
            seen[group] = true;
            sequence.push(group);
        }
    }
    sequence
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
    /// `input_routes` are the inner ports each development input feeds,
    /// by input index.
    pub(super) fn from_modules(
        module_list: Vec<(String, GraphModule)>,
        routing: &[RoutingConnection],
        input_routes: &[Vec<ExternalInputRoute>],
        grouping: Grouping,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let indexes = module_indexes_slice(&module_list);
        let routes = routes_of(routing, &indexes);
        let targets = targets_of(module_list.iter().map(|(_, m)| m), &indexes);
        let order = process_order(module_list.len(), &routes, &targets);
        let group_order = group_sequence(&order, &grouping.group_of, grouping.groups.len());
        let mut inner: Vec<Vec<InnerRoute>> = (0..module_list.len()).map(|_| Vec::new()).collect();

        for (input, targets) in input_routes.iter().enumerate() {
            for target in targets {
                inner[target.module_index].push(InnerRoute {
                    source: Source::Input(input),
                    to_port: target.port_index,
                    delayed: false,
                    first: false,
                });
            }
        }

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

            inner[to_module].push(InnerRoute {
                source: Source::Module {
                    module: from_module,
                    port: from_port,
                },
                to_port,
                delayed: is_back_edge(&grouping.group_of, &grouping.pos, from_module, to_module),
                first: false,
            });
        }

        // Each port's first route overwrites it; the rest add to it.
        for routes in &mut inner {
            for r in 0..routes.len() {
                let port = routes[r].to_port;
                routes[r].first = !routes[..r].iter().any(|route| route.to_port == port);
            }
        }

        let modules: Vec<GraphModule> = module_list.into_iter().map(|(_, m)| m).collect();

        // Per-internal-module output block buffers and carry storage.
        let out_counts: Vec<usize> = modules.iter().map(|m| m.module().outputs().len()).collect();
        let out_bufs: Vec<Vec<f32>> = out_counts
            .iter()
            .map(|&c| vec![0.0; c * MAX_BLOCK])
            .collect();
        let out_prev: Vec<Vec<f32>> = out_counts.iter().map(|&c| vec![0.0; c]).collect();

        Ok(Self {
            modules,
            routes: inner,
            groups: grouping.groups,
            group_order,
            out_counts,
            out_bufs,
            out_prev,
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
/// routing order) and ordering edges to each one's `targets`, which
/// [`group_sequence`] runs the groups in.
///
/// As at the top level (see `crate::invention::graph::compile`), a module
/// runs before the modules whose controls it writes (see
/// [`crate::Module::control_targets`]), so an inner scheduler's write lands
/// in the block it falls in. Unlike there, such an ordering edge that would
/// close a loop is left out rather than folded into a feedback group, so
/// only routing groups the modules (see [`Grouping`]): a scheduler writing
/// a module upstream of it (its own clock, say) lands its write a block
/// later, as before, and a payload retargeting it can reorder the groups
/// without regrouping them on the audio thread.
pub(super) fn process_order(
    n: usize,
    routes: &[(usize, usize)],
    targets: &[Vec<usize>],
) -> Vec<usize> {
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
    topological_order(&downstream)
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
