//! A table's clock. The graph's request drain plays a top-level module's
//! table of automation (a control scheduler's schedule) on the clock
//! output connected to its clock input (see
//! `crate::control_request::BeatTable`). So a table whose clock input has
//! any other source, or several, is refused: at load, by a change, and by
//! an offline edit. An unconnected clock input is left alone. Accepting any
//! gate waits on a condition that counts its rising edges (FUG-389).

use crate::invention::graph::{GraphCommand, RoutingConnection, SignalGraph};
use crate::invention::runtime::{GraphCommandError, ModuleInstance};
use crate::modules::clock::outputs::OUTPUTS;
use crate::Module;

use super::{MirrorModule, TopologyMirror};

/// The clock input `module` steps its table on, if it plays one, and its
/// outputs a table can step on: a clock's gates. Control thread.
pub(super) fn table_facts(module: &dyn Module) -> (Option<String>, Vec<String>) {
    let clock = module
        .beat_table()
        .map(|table| table.clock_input().to_string());
    let gates = module.outputs().iter();
    let gates = gates.filter(|port| module.gate_beats(port).is_some());
    (clock, gates.map(|port| (*port).to_string()).collect())
}

impl TopologyMirror {
    /// Refuses a table whose clock input has a source other than a clock's
    /// gate output, or more than one source.
    pub(crate) fn check_tables(&self) -> Result<(), String> {
        let tables = self.modules.iter();
        let tables = tables.filter_map(|(id, module)| Some((id, module.clock.as_deref()?)));
        for (id, input) in tables {
            let into = |edge: &&RoutingConnection| edge.to_module == *id && edge.to_port == input;
            let sources: Vec<_> = self.edges.iter().filter(into).collect();
            let named =
                |edge: &&RoutingConnection| format!("'{}.{}'", edge.from_module, edge.from_port);
            let connect = || {
                format!(
                    "module '{id}' steps on a clock: connect its '{input}' input to one \
                     clock gate output (a clock's {})",
                    clock_gates()
                )
            };
            match sources[..] {
                [] => {}
                [edge] if self.is_gate(edge) => {}
                [edge] => {
                    return Err(format!(
                        "{}, not {}. Inside a development it can step on any gate",
                        connect(),
                        named(&edge)
                    ))
                }
                [first, ..] if sources.iter().all(|edge| same_source(edge, first)) => {
                    return Err(format!(
                        "{}, once: {} is connected twice",
                        connect(),
                        named(&first)
                    ))
                }
                _ => {
                    let named: Vec<_> = sources.iter().map(named).collect();
                    return Err(format!("{}, not {}", connect(), named.join(" and ")));
                }
            }
        }
        Ok(())
    }

    fn is_gate(&self, edge: &RoutingConnection) -> bool {
        let source = self.modules.get(&edge.from_module);
        source.is_some_and(|source| source.gates.contains(&edge.from_port))
    }
}

/// A clock's gate outputs, quoted: "'beat', …, or 'beat_x4'".
fn clock_gates() -> String {
    let quoted: Vec<_> = OUTPUTS.iter().map(|output| format!("'{output}'")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Whether `a` and `b`, into one input, come from one output.
fn same_source(a: &RoutingConnection, b: &RoutingConnection) -> bool {
    a.from_module == b.from_module && a.from_port == b.from_port
}

/// Refuses an offline edit that would leave `graph` with a table on
/// another source: `module` added as, or replacing, `id`, or `edge` added.
/// An edge through a port its module lacks (one a replacement left) carries
/// nothing, so it is not counted, as a live edit removes it. The caller
/// holds the graph's lock across this and the edit.
pub(crate) fn check_offline(
    graph: &SignalGraph,
    module: Option<(&str, &ModuleInstance)>,
    edge: Option<&RoutingConnection>,
) -> Result<(), GraphCommandError> {
    // Only what the check reads: no targets, no timelines.
    let modules = graph.modules.iter();
    let modules = modules.map(|(id, instance)| (id.clone(), MirrorModule::of(instance)));
    let mut mirror = TopologyMirror {
        modules: modules.collect(),
        ..TopologyMirror::default()
    };
    if let Some((id, module)) = module {
        mirror
            .modules
            .insert(id.to_string(), MirrorModule::of(module));
    }
    let edges = graph.edges.iter().chain(edge);
    let carried = |edge: &&RoutingConnection| {
        let port = |id: &str| mirror.modules.get(id).map(|module| &module.ports);
        let from = port(&edge.from_module);
        let to = port(&edge.to_module);
        from.is_some_and(|ports| ports.outputs.contains(&edge.from_port))
            && to.is_some_and(|ports| ports.inputs.contains(&edge.to_port))
    };
    mirror.edges = edges.filter(carried).cloned().collect();
    mirror
        .check_tables()
        .map_err(GraphCommandError::InvalidPort)
}

/// Adds `edge` to an offline render's `graph`, refused if it would leave a
/// table on another source. The caller holds the graph's lock.
pub(crate) fn connect_offline(
    graph: &mut SignalGraph,
    edge: RoutingConnection,
) -> Result<(), GraphCommandError> {
    check_offline(graph, None, Some(&edge))?;
    graph.apply_command(GraphCommand::AddConnection {
        from_module: edge.from_module,
        from_port: edge.from_port,
        to_module: edge.to_module,
        to_port: edge.to_port,
    });
    Ok(())
}

/// Refuses a top-level invention about to load with a table on another
/// source (see [`TopologyMirror::check_tables`]).
pub(crate) fn check_load(
    modules: &indexmap::IndexMap<String, ModuleInstance>,
    edges: &[RoutingConnection],
) -> Result<(), String> {
    TopologyMirror::of(modules, edges).check_tables()
}

#[cfg(test)]
mod gates {
    #[test]
    fn the_refusal_names_every_clock_gate_output() {
        assert_eq!(
            super::clock_gates(),
            "'beat', 'beat_d4', 'beat_d2', 'beat_x2' or 'beat_x4'"
        );
    }
}
