//! Integration tests for the graph command queue (runtime mutation).

use fugue::invention::Invention;
use fugue::InventionBuilder;

fn build_simple_invention() -> fugue::RunningInvention {
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
    let (runtime, _handles) = builder.build(invention).expect("Failed to build invention");
    runtime.start().expect("Failed to start invention")
}

#[test]
fn test_set_module_input_succeeds() {
    let running = build_simple_invention();
    let result = running.set_module_input("osc", "frequency", 880.0);
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_set_nonexistent_module_succeeds() {
    // Commands are fire-and-forget; unknown modules are silently ignored on the audio thread.
    let running = build_simple_invention();
    let result = running.set_module_input("nonexistent", "frequency", 100.0);
    assert!(result.is_ok());
    running.stop();
}

#[test]
fn test_rapid_commands_all_succeed() {
    let running = build_simple_invention();
    for i in 0..100 {
        let freq = 200.0 + i as f32;
        let result = running.set_module_input("osc", "frequency", freq);
        assert!(result.is_ok(), "Command {} failed", i);
    }
    running.stop();
}
