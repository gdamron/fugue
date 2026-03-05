//! Integration tests for the graph command queue (runtime mutation).

use fugue::invention::Invention;
use fugue::InventionBuilder;

fn build_simple_invention() -> (fugue::RunningInvention, fugue::InventionHandles) {
    let json = r#"
    {
        "name": "Command Test",
        "modules": [
            {
                "id": "osc",
                "type": "oscillator",
                "config": {"frequency": 440.0}
            },
            {
                "id": "dac",
                "type": "dac"
            }
        ],
        "connections": [
            {"from": "osc", "from_port": "audio", "to": "dac", "to_port": "audio"}
        ]
    }
    "#;

    let invention: Invention = serde_json::from_str(json).expect("Failed to parse invention");
    let builder = InventionBuilder::new(44100);
    let (runtime, handles) = builder.build(invention).expect("Failed to build invention");
    let running = runtime.start().expect("Failed to start invention");
    (running, handles)
}

#[test]
fn test_set_module_input_succeeds() {
    let (running, _handles) = build_simple_invention();
    let result = running.set_module_input("osc", "frequency", 880.0);
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_set_nonexistent_module_succeeds() {
    // Commands are fire-and-forget; unknown modules are silently ignored on the audio thread.
    let (running, _handles) = build_simple_invention();
    let result = running.set_module_input("nonexistent", "frequency", 100.0);
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_rapid_commands_all_succeed() {
    let (running, _handles) = build_simple_invention();
    for i in 0..100 {
        let freq = 200.0 + i as f32;
        let result = running.set_module_input("osc", "frequency", freq);
        assert!(result.is_ok(), "Command {} failed", i);
    }
    running.stop();
}

// --- Module lifecycle tests ---

#[test]
fn test_add_module_succeeds() {
    let (running, _handles) = build_simple_invention();
    let config = serde_json::json!({"frequency": 220.0});
    let result = running.add_module("osc2", "oscillator", &config);
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_add_module_returns_handles() {
    let (running, _handles) = build_simple_invention();
    let config = serde_json::json!({});
    let handles = running
        .add_module("clock2", "clock", &config)
        .expect("Failed to add clock module");
    let controls: Option<fugue::ClockControls> = handles.get("clock2.controls");
    assert!(controls.is_some(), "Expected clock controls handle");
    running.stop();
}

#[test]
fn test_add_module_unknown_type_fails() {
    let (running, _handles) = build_simple_invention();
    let config = serde_json::json!({});
    let result = running.add_module("x", "nonexistent", &config);
    assert!(result.is_err());
    match result.unwrap_err() {
        fugue::GraphCommandError::UnknownModuleType(t) => assert_eq!(t, "nonexistent"),
        other => panic!("Expected UnknownModuleType, got: {:?}", other),
    }
    running.stop();
}

#[test]
fn test_remove_module_succeeds() {
    let (running, _handles) = build_simple_invention();
    let result = running.remove_module("osc");
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_remove_nonexistent_module_succeeds() {
    // Fire-and-forget: removing a nonexistent module is fine.
    let (running, _handles) = build_simple_invention();
    let result = running.remove_module("does_not_exist");
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_add_then_remove_module() {
    let (running, _handles) = build_simple_invention();
    let config = serde_json::json!({"frequency": 330.0});
    let add_result = running.add_module("osc3", "oscillator", &config);
    assert!(add_result.is_ok());
    let remove_result = running.remove_module("osc3");
    assert!(remove_result.is_ok());
    running.stop();
}

// --- Connection management tests ---

#[test]
fn test_connect_succeeds() {
    let (running, _handles) = build_simple_invention();
    let result = running.connect("osc", "audio", "dac", "audio");
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_connect_unknown_source_module_fails() {
    let (running, _handles) = build_simple_invention();
    let result = running.connect("nonexistent", "audio", "dac", "audio");
    assert!(result.is_err());
    match result.unwrap_err() {
        fugue::GraphCommandError::UnknownModule(id) => assert_eq!(id, "nonexistent"),
        other => panic!("Expected UnknownModule, got: {:?}", other),
    }
    running.stop();
}

#[test]
fn test_connect_unknown_dest_module_fails() {
    let (running, _handles) = build_simple_invention();
    let result = running.connect("osc", "audio", "nonexistent", "audio");
    assert!(result.is_err());
    match result.unwrap_err() {
        fugue::GraphCommandError::UnknownModule(id) => assert_eq!(id, "nonexistent"),
        other => panic!("Expected UnknownModule, got: {:?}", other),
    }
    running.stop();
}

#[test]
fn test_connect_invalid_output_port_fails() {
    let (running, _handles) = build_simple_invention();
    let result = running.connect("osc", "bad_port", "dac", "audio");
    assert!(result.is_err());
    match result.unwrap_err() {
        fugue::GraphCommandError::InvalidPort(msg) => {
            assert!(msg.contains("bad_port"), "Error should mention the bad port: {}", msg);
        }
        other => panic!("Expected InvalidPort, got: {:?}", other),
    }
    running.stop();
}

#[test]
fn test_connect_invalid_input_port_fails() {
    let (running, _handles) = build_simple_invention();
    let result = running.connect("osc", "audio", "dac", "bad_port");
    assert!(result.is_err());
    match result.unwrap_err() {
        fugue::GraphCommandError::InvalidPort(msg) => {
            assert!(msg.contains("bad_port"), "Error should mention the bad port: {}", msg);
        }
        other => panic!("Expected InvalidPort, got: {:?}", other),
    }
    running.stop();
}

#[test]
fn test_disconnect_succeeds() {
    let (running, _handles) = build_simple_invention();
    let result = running.disconnect("osc", "audio", "dac", "audio");
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_disconnect_nonexistent_succeeds() {
    let (running, _handles) = build_simple_invention();
    let result = running.disconnect("nonexistent", "audio", "dac", "audio");
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_add_module_then_connect() {
    let (running, _handles) = build_simple_invention();
    let config = serde_json::json!({"frequency": 220.0});
    let _handles2 = running
        .add_module("osc2", "oscillator", &config)
        .expect("Failed to add oscillator");
    // Wait for the audio thread to process the AddModule command
    // so that connect() can find the module in the graph.
    std::thread::sleep(std::time::Duration::from_millis(50));
    let result = running.connect("osc2", "audio", "dac", "audio");
    assert!(result.is_ok());
    running.stop();
}
