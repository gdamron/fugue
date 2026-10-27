//! The agent's `trigger` and `reset` event controls: each write is one
//! edge, counted without allocating, and none is authored or replayed.

use super::AgentControls;
use crate::alloc_counter::allocator_events;
use crate::invention::manual_backend::{start_manual, Pump, SAMPLE_RATE};
use crate::{ControlValue, ControlWriteIntent, Invention, InventionBuilder, RunningInvention};

/// A disabled agent, so no request runs while its edges are counted, with
/// `extra` appended to its config.
fn agent_doc(extra: &str) -> Invention {
    Invention::from_json(&format!(
        r#"{{
        "version": "1.0.0",
        "modules": [
            {{ "id": "agent", "type": "agent",
               "config": {{ "enabled": false, "backend": "test:echo"{extra} }} }},
            {{ "id": "dac", "type": "dac" }}
        ],
        "connections": []
    }}"#
    ))
    .unwrap()
}

fn started(extra: &str) -> (RunningInvention, Pump, AgentControls) {
    let (runtime, handles) = InventionBuilder::new(SAMPLE_RATE)
        .build(agent_doc(extra))
        .unwrap();
    let agent: AgentControls = handles.get("agent.controls").unwrap();
    let (running, pump) = start_manual(runtime);
    pump.render(2);
    (running, pump, agent)
}

fn counts(agent: &AgentControls) -> (u32, u32) {
    (agent.trigger_count(), agent.reset_count())
}

fn perform(running: &RunningInvention, key: &str, fire: bool) {
    let (value, intent) = (fire.into(), ControlWriteIntent::Perform);
    let snapshot = running.snapshot();
    snapshot
        .set_control_with_intent("agent", key, value, intent)
        .unwrap();
}

#[test]
fn each_write_is_one_edge_counted_without_allocating() {
    let (running, pump, agent) = started("");
    // Two triggers and a reset, all landing in one block, and a `false`
    // that fires nothing.
    perform(&running, "trigger", true);
    perform(&running, "trigger", true);
    perform(&running, "trigger", false);
    perform(&running, "reset", true);
    let ((), allocs, frees) = allocator_events(|| pump.block());
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(counts(&agent), (2, 1));
    // The cells never hold a fired event.
    for key in ["trigger", "reset"] {
        let read = running.get_control("agent", key).unwrap();
        assert_eq!(read, ControlValue::Bool(false), "{key}");
    }
}

#[test]
fn an_authored_trigger_fires_once_and_is_never_replayed() {
    let (mut running, pump, agent) = started("");
    // The wire's default intent is authoring.
    running
        .set_control("agent", "trigger", ControlValue::Bool(true))
        .unwrap();
    pump.render(2);
    assert_eq!(counts(&agent), (1, 0));

    let json = running.document().unwrap().to_json().unwrap();
    assert!(!json.contains("trigger"), "the document holds no event key");
    running
        .reload(Invention::from_json(&json).unwrap())
        .unwrap();
    // Nor does a hand-written key fire, whatever it is changed to.
    for extra in [
        r#", "trigger": true, "reset": true"#,
        r#", "trigger": false"#,
    ] {
        let report = running.reload(agent_doc(extra)).unwrap();
        assert!(report.controls_updated.is_empty(), "{report:?}");
        assert!(report.swapped.is_empty(), "{report:?}");
    }
    pump.render(2);
    assert_eq!(counts(&agent), (1, 0));
}

#[test]
fn a_saved_trigger_key_does_not_fire_as_the_agent_loads() {
    let (_running, pump, agent) = started(r#", "trigger": true, "reset": true"#);
    pump.render(2);
    assert_eq!(counts(&agent), (0, 0));
}
