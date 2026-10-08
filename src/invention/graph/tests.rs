use super::*;

/// Creates a minimal SignalGraph for testing process order.
/// Modules are stub oscillators — we only care about the topology.
fn test_graph(module_ids: &[&str], connections: &[(&str, &str)]) -> SignalGraph {
    let registry = crate::ModuleRegistry::default();
    let null_config = serde_json::Value::Null;

    let mut modules = IndexMap::new();
    for &id in module_ids {
        let result = registry
            .build("oscillator", 44100, &null_config)
            .expect("oscillator is a valid type");
        modules.insert(id.to_string(), result.module);
    }

    let edges: Vec<RoutingConnection> = connections
        .iter()
        .map(|&(from, to)| RoutingConnection {
            from_module: from.to_string(),
            from_port: "audio".to_string(),
            to_module: to.to_string(),
            to_port: "frequency_mod".to_string(),
        })
        .collect();

    let mut graph = SignalGraph::new(
        modules,
        Vec::new(),
        edges,
        super::MasterObservers::default(),
    );
    graph.recompile();
    graph
}

/// Returns the feedback flag for the group containing `id`.
fn group_is_feedback(graph: &SignalGraph, id: &str) -> bool {
    let idx = graph
        .modules
        .get_index_of(id)
        .unwrap_or_else(|| panic!("{id} not found"));
    graph
        .process_groups
        .iter()
        .find(|g| g.members.contains(&idx))
        .map(|g| g.feedback)
        .unwrap_or_else(|| panic!("{id} not in any process group"))
}

/// Helper: returns the position of `id` in the process order.
fn pos(graph: &SignalGraph, id: &str) -> usize {
    graph
        .process_order_names()
        .iter()
        .position(|s| s == id)
        .unwrap_or_else(|| panic!("{id} not found in process_order"))
}

#[test]
fn test_cycle_downstream_ordering() {
    // osc1 ↔ osc2 (cycle), osc1 → dac
    // All three nodes must be visited (Kahn's would strand dac).
    // dac must appear after osc1 (its direct dependency).
    let graph = test_graph(
        &["osc1", "osc2", "dac"],
        &[("osc1", "osc2"), ("osc2", "osc1"), ("osc1", "dac")],
    );

    let order = graph.process_order_names();
    assert_eq!(order.len(), 3, "all modules must be visited");
    assert!(
        pos(&graph, "dac") > pos(&graph, "osc1"),
        "dac must come after its dependency osc1, got order: {:?}",
        order,
    );
}

#[test]
fn test_linear_chain_ordering() {
    // a → b → c — strict dependency order
    let graph = test_graph(&["a", "b", "c"], &[("a", "b"), ("b", "c")]);
    assert_eq!(graph.process_order_names(), vec!["a", "b", "c"]);
}

#[test]
fn test_three_node_cycle_all_visited() {
    // a → b → c → a (cycle) — all three must appear in process order
    let graph = test_graph(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("c", "a")]);
    let order = graph.process_order_names();
    assert_eq!(order.len(), 3);
    // a should come before b, b before c (back-edge is c→a)
    assert!(pos(&graph, "a") < pos(&graph, "b"), "order: {:?}", order);
    assert!(pos(&graph, "b") < pos(&graph, "c"), "order: {:?}", order);
}

#[test]
fn test_acyclic_modules_are_not_feedback_groups() {
    // a → b → c — every module is its own trivial, full-block group.
    let graph = test_graph(&["a", "b", "c"], &[("a", "b"), ("b", "c")]);
    assert_eq!(graph.process_groups.len(), 3);
    assert!(!group_is_feedback(&graph, "a"));
    assert!(!group_is_feedback(&graph, "b"));
    assert!(!group_is_feedback(&graph, "c"));
}

#[test]
fn test_cycle_members_share_one_feedback_group() {
    // osc1 ↔ osc2 (cycle), osc1 → dac.
    let graph = test_graph(
        &["osc1", "osc2", "dac"],
        &[("osc1", "osc2"), ("osc2", "osc1"), ("osc1", "dac")],
    );

    // osc1 and osc2 form a single feedback group; dac is acyclic.
    assert!(group_is_feedback(&graph, "osc1"));
    assert!(group_is_feedback(&graph, "osc2"));
    assert!(!group_is_feedback(&graph, "dac"));

    let osc1 = graph.modules.get_index_of("osc1").unwrap();
    let osc2 = graph.modules.get_index_of("osc2").unwrap();
    let group = graph
        .process_groups
        .iter()
        .find(|g| g.members.contains(&osc1))
        .unwrap();
    assert!(group.members.contains(&osc2));
    assert_eq!(group.members.len(), 2);

    // Exactly one of the two intra-cycle edges is a delayed back-edge.
    let delayed_into_osc1 = graph.compiled_routes[osc1].iter().any(|r| r.delayed);
    let delayed_into_osc2 = graph.compiled_routes[osc2].iter().any(|r| r.delayed);
    assert!(
        delayed_into_osc1 ^ delayed_into_osc2,
        "exactly one back-edge expected in a 2-cycle"
    );
}

#[test]
fn test_three_node_cycle_is_single_feedback_group() {
    let graph = test_graph(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("c", "a")]);
    assert_eq!(graph.process_groups.len(), 1);
    assert!(graph.process_groups[0].feedback);
    assert_eq!(graph.process_groups[0].members.len(), 3);
}

/// A live graph compiles its process order on the control thread from the
/// modules' control surfaces; offline render compiles it from the modules.
/// Every built-in type must report the same control targets both ways.
#[test]
fn every_module_type_reports_its_control_targets_on_its_surface() {
    let registry = crate::ModuleRegistry::default();
    let mut checked = Vec::new();
    for type_id in registry.types() {
        if registry.is_sink(type_id) {
            // Building a sink may open a file or stream; sinks expose no
            // control surface.
            continue;
        }
        let Ok(result) = registry.build(type_id, 48_000, &serde_json::json!({})) else {
            continue;
        };
        let from_module = result.module.module().control_targets();
        let from_surface = result
            .control_surface
            .map(|surface| surface.control_targets())
            .unwrap_or_default();
        assert_eq!(from_module, from_surface, "control targets of '{type_id}'");
        checked.push(type_id.to_string());
    }
    for expected in ["control_scheduler", "oscillator", "clock", "mixer", "code"] {
        assert!(
            checked.iter().any(|t| t == expected),
            "'{expected}' was not checked (checked: {checked:?})"
        );
    }
}
