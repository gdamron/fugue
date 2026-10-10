mod support;

use std::thread;
use std::time::{Duration, Instant};

use fugue::{AgentControls, ControlValue, Invention, InventionBuilder};
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
                        "root_note": 36,
                        "step_count": 4,
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
                                "control": "pattern",
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

    let (runtime, handles) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(48_000))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let agent: AgentControls = handles.get("agent.controls").unwrap();
    loop {
        // A rising edge on the `trigger` input.
        agent.increment_trigger();

        let count = running.get_control("agent", "request_count").unwrap();
        if let ControlValue::Number(count) = count {
            if count >= 1.0 {
                break;
            }
        }
        assert!(Instant::now() < deadline, "agent did not complete request");
        thread::sleep(Duration::from_millis(20));
    }

    let pattern_json = running.get_control("bass_seq", "pattern").unwrap();
    let ControlValue::String(pattern_json) = pattern_json else {
        panic!("pattern should be a string control");
    };
    let pattern: serde_json::Value = serde_json::from_str(&pattern_json).unwrap();
    assert_eq!(pattern.as_array().unwrap().len(), 3);
    assert_eq!(pattern[1]["note"], 3);

    running.stop();
}

#[test]
fn agent_apply_preflights_all_paths_before_writing() {
    let invention = Invention::from_json(
        r#"{
            "version": "1.0.0",
            "modules": [
                {
                    "id": "bass_seq",
                    "type": "step_sequencer",
                    "config": {
                        "root_note": 36,
                        "step_count": 4,
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
                                "control": "pattern",
                                "type": "json_string"
                            },
                            {
                                "from": "$.payload.missing",
                                "to": "bass_seq",
                                "control": "step_count",
                                "type": "number"
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

    let (runtime, handles) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(48_000))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let agent: AgentControls = handles.get("agent.controls").unwrap();
    loop {
        // A rising edge on the `trigger` input.
        agent.increment_trigger();

        let error = running.get_control("agent", "last_error").unwrap();
        if let ControlValue::String(error) = error {
            if error.contains("apply path '$.payload.missing' not found") {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "agent did not report apply error"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let pattern_json = running.get_control("bass_seq", "pattern").unwrap();
    let ControlValue::String(pattern_json) = pattern_json else {
        panic!("pattern should be a string control");
    };
    let pattern: serde_json::Value = serde_json::from_str(&pattern_json).unwrap();
    assert_eq!(pattern.as_array().unwrap().len(), 4);
    assert_eq!(pattern[1]["note"], serde_json::Value::Null);

    running.stop();
}

#[test]
fn step_sequencer_pattern_round_trips() {
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
            "pattern",
            ControlValue::String(r#"[{"note":0,"gate":0.5},{"note":null}]"#.to_string()),
        )
        .unwrap();
    let value = running.get_control("seq", "pattern").unwrap();
    let ControlValue::String(value) = value else {
        panic!("pattern should be a string control");
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
    let (runtime, handles) = InventionBuilder::new(48_000).build(invention).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(48_000))
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let agent: AgentControls = handles.get("agent.controls").unwrap();
    loop {
        // A rising edge on the `trigger` input.
        agent.increment_trigger();

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

/// An agent that answers every trigger with a canned JSON response, keeping
/// at most `max_turns` of history.
fn echo_agent(max_turns: u64) -> Invention {
    Invention::from_json(&format!(
        r#"{{
            "version": "1.0.0",
            "modules": [
                {{
                    "id": "agent",
                    "type": "agent",
                    "config": {{
                        "backend": "test:response",
                        "prompt": "Answer.",
                        "system_prompt": "You answer.",
                        "history_limits": {{ "max_turns": {max_turns} }},
                        "response": {{ "format": "json" }},
                        "test_response": {{
                            "kind": "echo",
                            "summary": "an echo",
                            "payload": {{}},
                            "confidence": 1.0,
                            "warnings": []
                        }}
                    }}
                }},
                {{ "id": "dac", "type": "dac" }}
            ],
            "connections": []
        }}"#
    ))
    .unwrap()
}

/// Triggers the agent until it has completed `requests` requests.
fn complete_requests(running: &fugue::RunningInvention, agent: &AgentControls, requests: f32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ControlValue::Number(count) = running.get_control("agent", "request_count").unwrap()
        else {
            panic!("request_count is a number");
        };
        if count >= requests {
            return;
        }
        agent.increment_trigger();
        assert!(
            Instant::now() < deadline,
            "agent completed {count} requests"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn agent_telemetry_is_read_only_through_set_control() {
    let (runtime, handles) = InventionBuilder::new(48_000).build(echo_agent(6)).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(48_000))
        .unwrap();
    let agent: AgentControls = handles.get("agent.controls").unwrap();
    complete_requests(&running, &agent, 1.0);

    let ControlValue::String(parsed) = running
        .get_control("agent", "last_parsed_response")
        .unwrap()
    else {
        panic!("last_parsed_response is a string");
    };
    let parsed: serde_json::Value = serde_json::from_str(&parsed).unwrap();
    assert_eq!(parsed["summary"], "an echo");
    for (key, value) in [
        ("status", ControlValue::String("written".into())),
        ("last_error", ControlValue::String("written".into())),
        ("last_response", ControlValue::String("written".into())),
        ("last_parsed_response", ControlValue::String("{}".into())),
        ("history", ControlValue::String("[]".into())),
        ("last_apply_error", ControlValue::String("written".into())),
        ("request_count", ControlValue::Number(99.0)),
    ] {
        let before = running.get_control("agent", key).unwrap();
        let refusal = running.set_control("agent", key, value).unwrap_err();
        assert!(
            refusal.to_string().contains("is read-only"),
            "{key}: {refusal}"
        );
        assert_eq!(running.get_control("agent", key).unwrap(), before, "{key}");
    }
    // Parameters stay writable.
    running
        .set_control("agent", "cooldown", ControlValue::Number(0.5))
        .unwrap();
    running.stop();
}

#[test]
fn history_limits_bound_the_history() {
    let (runtime, handles) = InventionBuilder::new(48_000).build(echo_agent(1)).unwrap();
    let running = runtime
        .start_with_backend(NullAudioBackend::new(48_000))
        .unwrap();
    let agent: AgentControls = handles.get("agent.controls").unwrap();
    complete_requests(&running, &agent, 2.0);

    let ControlValue::String(history) = running.get_control("agent", "history").unwrap() else {
        panic!("history is a string");
    };
    let history: serde_json::Value = serde_json::from_str(&history).unwrap();
    assert_eq!(history.as_array().unwrap().len(), 1, "{history}");
    assert_eq!(history[0]["request"]["system"], "You answer.");
    running.stop();
}
