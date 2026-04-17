mod support;

use std::thread;
use std::time::{Duration, Instant};

use fugue::{ControlValue, Invention, InventionBuilder};
use support::NullAudioBackend;

#[test]
fn agent_trigger_applies_step_pattern_response() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "bass_seq",
                    "type": "step_sequencer",
                    "config": {
                        "base_note": 36,
                        "steps": 4,
                        "pattern": [
                            { "note": 0 },
                            { "note": null },
                            { "note": 7 },
                            { "note": 5 }
                        ]
                    }
                },
                {
                    "id": "agent",
                    "type": "agent",
                    "config": {
                        "backend": "test:response",
                        "prompt": "Generate a variation of the current motif.",
                        "include_graph_summary": true,
                        "context": [
                            {
                                "name": "current_motif",
                                "from": "bass_seq",
                                "source": "config",
                                "path": "pattern"
                            }
                        ],
                        "response": {
                            "format": "json",
                            "kind": "pattern_variation",
                            "schema_ref": "fugue.step_pattern.v1"
                        },
                        "test_response": {
                            "kind": "pattern_variation",
                            "summary": "test variation",
                            "payload": {
                                "pattern": [
                                    { "note": 0, "gate": 0.75 },
                                    { "note": 3, "gate": 0.5 },
                                    { "note": null }
                                ]
                            },
                            "confidence": 1.0,
                            "warnings": []
                        },
                        "apply": [
                            {
                                "from": "$.payload.pattern",
                                "to": "bass_seq",
                                "control": "pattern_json",
                                "type": "json_string"
                            }
                        ]
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
    )
    .unwrap();

    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(48_000))
        .unwrap();

    running.set_module_input("agent", "trigger", 1.0).unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let count = running.get_control("agent", "request_count").unwrap();
        if count == ControlValue::Number(1.0) {
            break;
        }
        assert!(Instant::now() < deadline, "agent did not complete request");
        thread::sleep(Duration::from_millis(20));
    }

    let pattern_json = running.get_control("bass_seq", "pattern_json").unwrap();
    let ControlValue::String(pattern_json) = pattern_json else {
        panic!("pattern_json should be a string control");
    };
    let pattern: serde_json::Value = serde_json::from_str(&pattern_json).unwrap();
    assert_eq!(pattern.as_array().unwrap().len(), 3);
    assert_eq!(pattern[1]["note"], 3);

    running.stop();
}

#[test]
fn step_sequencer_pattern_json_round_trips() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                { "id": "seq", "type": "step_sequencer", "config": {} },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
    )
    .unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(48_000))
        .unwrap();

    running
        .set_control(
            "seq",
            "pattern_json",
            ControlValue::String(r#"[{"note":0,"gate":0.5},{"note":null}]"#.to_string()),
        )
        .unwrap();
    let value = running.get_control("seq", "pattern_json").unwrap();
    let ControlValue::String(value) = value else {
        panic!("pattern_json should be a string control");
    };
    let parsed: serde_json::Value = serde_json::from_str(&value).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 2);

    running.stop();
}

#[test]
fn named_local_harness_reports_missing_command_cleanly() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "agent",
                    "type": "agent",
                    "config": {
                        "backend": "local:__missing_harness_for_test__",
                        "prompt": "test"
                    }
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        }"#,
    )
    .unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(48_000))
        .unwrap();

    running.set_module_input("agent", "trigger", 1.0).unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let error = running.get_control("agent", "last_error").unwrap();
        if let ControlValue::String(error) = error {
            if error.contains("unknown local agent harness") {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "agent did not report backend error"
        );
        thread::sleep(Duration::from_millis(20));
    }

    running.stop();
}
