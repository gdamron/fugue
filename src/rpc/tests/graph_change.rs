use super::*;
use crate::RuntimeConnectionInfo;
use serde_json::json;

fn rev(n: u64) -> RuntimeRevision {
    RuntimeRevision {
        session_id: "session".into(),
        revision: n,
    }
}

fn module(id: &str, module_type: &str) -> ChangedModule {
    ChangedModule {
        id: id.into(),
        module_type: module_type.into(),
    }
}

fn wire(from: &str, from_port: &str, to: &str, to_port: &str) -> RuntimeConnectionInfo {
    RuntimeConnectionInfo {
        from: from.into(),
        from_port: from_port.into(),
        to: to.into(),
        to_port: to_port.into(),
    }
}

/// A voice: oscillator into a mixer, an LFO on the oscillator's pitch.
fn voice() -> GraphShape {
    GraphShape {
        modules: vec![
            module("osc", "oscillator"),
            module("lfo", "lfo"),
            module("mix", "mixer"),
        ],
        connections: vec![
            wire("osc", "out", "mix", "in1"),
            wire("lfo", "out", "osc", "fm"),
        ],
    }
}

fn change(before: &GraphShape, after: &GraphShape) -> Option<GraphChange> {
    GraphChange::between(before, after, rev(4), rev(5))
}

#[test]
fn an_unchanged_shape_is_no_change() {
    assert_eq!(change(&voice(), &voice()), None);
}

#[test]
fn an_inserted_filter_is_added_and_rewires_both_ends() {
    let mut after = voice();
    after.modules.insert(1, module("filter", "filter"));
    after.connections[0] = wire("osc", "out", "filter", "in");
    after.connections.push(wire("filter", "out", "mix", "in1"));
    let c = change(&voice(), &after).unwrap();
    assert_eq!(c.added, vec![module("filter", "filter")]);
    assert!(c.removed.is_empty() && c.replaced.is_empty());
    assert_eq!(c.rewired, vec!["osc", "mix"]);
    assert_eq!(
        c.connections_added,
        vec![
            wire("osc", "out", "filter", "in"),
            wire("filter", "out", "mix", "in1")
        ]
    );
    assert_eq!(
        c.connections_removed,
        vec![wire("osc", "out", "mix", "in1")]
    );
    assert_eq!(c.change_count, 6);
    assert_eq!((c.from, c.to), (rev(4), rev(5)));
}

#[test]
fn a_removed_module_takes_its_connections_and_rewires_what_it_fed() {
    let mut after = voice();
    after.modules.remove(1);
    after.connections.remove(1);
    let c = change(&voice(), &after).unwrap();
    assert_eq!(c.removed, vec![module("lfo", "lfo")]);
    assert_eq!(c.connections_removed, vec![wire("lfo", "out", "osc", "fm")]);
    // The removed module itself is not rewired; the one it fed is.
    assert_eq!(c.rewired, vec!["osc"]);
    assert!(c.added.is_empty() && c.connections_added.is_empty());
    assert_eq!(c.change_count, 3);
}

#[test]
fn a_rewired_connection_names_every_module_it_touched() {
    let mut after = voice();
    after.connections[1] = wire("lfo", "out", "mix", "level1");
    let c = change(&voice(), &after).unwrap();
    assert!(c.added.is_empty() && c.removed.is_empty() && c.replaced.is_empty());
    assert_eq!(c.rewired, vec!["osc", "lfo", "mix"]);
    assert_eq!(
        c.connections_added,
        vec![wire("lfo", "out", "mix", "level1")]
    );
    assert_eq!(c.connections_removed, vec![wire("lfo", "out", "osc", "fm")]);
}

#[test]
fn a_batch_lists_exactly_its_insert_removal_and_rewiring() {
    // Insert a filter, remove the LFO, and add a delay send from the mixer.
    let after = GraphShape {
        modules: vec![
            module("osc", "oscillator"),
            module("filter", "filter"),
            module("mix", "mixer"),
            module("delay", "delay"),
        ],
        connections: vec![
            wire("osc", "out", "filter", "in"),
            wire("filter", "out", "mix", "in1"),
            wire("mix", "out", "delay", "in"),
        ],
    };
    let c = change(&voice(), &after).unwrap();
    assert_eq!(
        c.added,
        vec![module("filter", "filter"), module("delay", "delay")]
    );
    assert_eq!(c.removed, vec![module("lfo", "lfo")]);
    assert_eq!(c.rewired, vec!["osc", "mix"]);
    assert_eq!(c.connections_added, after.connections);
    assert_eq!(c.connections_removed, voice().connections);
    assert_eq!(c.change_count, 3 + 2 + 3 + 2);
}

#[test]
fn a_new_type_under_the_same_id_is_replaced_not_rewired() {
    let mut after = voice();
    after.modules[0] = module("osc", "wavetable");
    after.connections.remove(1);
    let c = change(&voice(), &after).unwrap();
    assert_eq!(c.replaced, vec![module("osc", "wavetable")]);
    assert!(c.added.is_empty() && c.removed.is_empty());
    // The LFO lost its only connection; the replaced oscillator is not
    // listed twice.
    assert_eq!(c.rewired, vec!["lfo"]);
}

#[test]
fn a_change_too_large_to_list_says_so_with_its_size() {
    let after = GraphShape {
        modules: (0..MAX_GRAPH_CHANGE_ITEMS)
            .map(|i| module(&format!("m{i}"), "oscillator"))
            .collect(),
        connections: Vec::new(),
    };
    let c = change(&GraphShape::default(), &after).unwrap();
    assert!(!c.too_large);
    assert_eq!(c.added.len(), MAX_GRAPH_CHANGE_ITEMS);

    let c = change(&voice(), &after).unwrap();
    assert!(c.too_large);
    assert_eq!(c.change_count, MAX_GRAPH_CHANGE_ITEMS + 3 + 2);
    assert!(c.added.is_empty() && c.removed.is_empty() && c.connections_removed.is_empty());
}

#[test]
fn the_wire_carries_only_what_changed() {
    let mut after = voice();
    after.modules.remove(1);
    after.connections.remove(1);
    let c = change(&voice(), &after).unwrap();
    let event =
        serde_json::to_value(RpcEvent::new(RpcEventPayload::GraphChanged(c.clone()))).unwrap();
    assert_eq!(
        event,
        json!({
            "schema_version": RPC_SCHEMA_VERSION,
            "event": "graph_changed",
            "from": { "session_id": "session", "revision": 4 },
            "to": { "session_id": "session", "revision": 5 },
            "removed": [{ "id": "lfo", "module_type": "lfo" }],
            "rewired": ["osc"],
            "connections_removed": [
                { "from": "lfo", "from_port": "out", "to": "osc", "to_port": "fm" }
            ],
            "change_count": 3
        })
    );
    let back: RpcEvent = serde_json::from_value(event).unwrap();
    assert_eq!(back.payload, RpcEventPayload::GraphChanged(c.clone()));

    let reply = serde_json::to_value(RpcResponsePayload::GraphChange { change: None }).unwrap();
    assert_eq!(reply, json!({ "kind": "graph_change", "change": null }));
}

#[test]
fn reading_the_latest_change_is_resent_freely() {
    let request: RpcRequest = serde_json::from_value(json!({
        "schema_version": RPC_SCHEMA_VERSION, "kind": "get_graph_change"
    }))
    .unwrap();
    assert_eq!(request.payload, RpcRequestPayload::GetGraphChange);
    assert_eq!(request.payload.replay_policy(), ReplayPolicy::Resend);
}

#[test]
fn a_snapshot_has_the_shape_of_its_graph() {
    let snapshot: RuntimeFullSnapshot = serde_json::from_value(json!({
        "status": { "running": true, "sample_rate": 48000, "module_count": 1, "connection_count": 0 },
        "modules": [{
            "info": { "id": "osc", "module_type": "oscillator", "config": {} },
            "ports": { "inputs": [], "outputs": ["out"] }
        }],
        "connections": []
    }))
    .unwrap();
    assert_eq!(
        GraphShape::of_snapshot(&snapshot),
        GraphShape {
            modules: vec![module("osc", "oscillator")],
            connections: Vec::new(),
        }
    );
}
