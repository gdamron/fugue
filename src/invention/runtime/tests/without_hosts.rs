//! A piece started without hosts runs no script and no agent worker.

use super::*;

/// A piece with a script that would add a module, and an agent.
fn hosted_piece() -> InventionRuntime {
    let invention = Invention::from_json(
        r#"{
            "modules": [
                {"id": "osc", "type": "oscillator"},
                {"id": "script", "type": "code", "config": {
                    "script": "function init() { graph.addModule('ran', 'oscillator', {}) }"
                }},
                {"id": "helper", "type": "agent"}
            ],
            "connections": []
        }"#,
    )
    .unwrap();
    InventionBuilder::new(48_000).build(invention).unwrap().0
}

fn rendered(running: &RunningInvention) -> bool {
    wait_until(|| {
        running
            .status()
            .diagnostics
            .as_ref()
            .is_some_and(|diagnostics| diagnostics.callback_count > 0)
    })
}

fn script_status(running: &RunningInvention) -> ControlValue {
    running.get_control("script", "status").unwrap()
}

#[test]
fn started_without_hosts_no_script_or_agent_runs() {
    let running = hosted_piece()
        .start_without_hosts(TickBackend::new(48_000))
        .unwrap();
    assert!(rendered(&running));
    assert_eq!(running.scripts.running(), 0);
    assert_eq!(running.agents.running(), 0);
    // The modules are built and keep their controls; the script never ran.
    assert_eq!(script_status(&running), ControlValue::String("idle".into()));
    assert!(running.get_control("helper", "status").is_ok());
    assert!(!running
        .list_modules()
        .iter()
        .any(|module| module.id == "ran"));

    // Nor does a module a later edit adds start one.
    running
        .add_module("late", "code", &serde_json::json!({"script": ""}))
        .unwrap();
    running
        .add_module("late_agent", "agent", &serde_json::json!({}))
        .unwrap();
    assert_eq!(running.scripts.running(), 0);
    assert_eq!(running.agents.running(), 0);
    running.stop();
}

#[test]
fn started_with_hosts_the_same_piece_runs_both() {
    let running = hosted_piece()
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();
    assert_eq!(running.scripts.running(), 1);
    assert_eq!(running.agents.running(), 1);
    assert!(wait_for_module(&running, "ran"));
    running.stop();
}
