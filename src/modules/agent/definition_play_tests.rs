//! A running agent with a definition, as its requests and the piece hear
//! it: the prompt it sends, the version it resolved, and what a reload or a
//! refused change leaves playing.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{install, install_pulse_agent, piece, start, text, with_pulse_agent};
use super::{PULSE_AGENT, REFERENCE};
use crate::pkg::audio_asset::with_packs_dir;
use crate::{ControlValue, Invention, InventionBuilder, NullBackend, RunningInvention};

/// Starts `invention` with no audio device.
fn play(invention: Invention) -> RunningInvention {
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    runtime
        .start_with_backend(NullBackend::new(48_000))
        .unwrap()
}

/// Triggers one request of `module` and waits for it to complete.
fn request(running: &RunningInvention, module: &str) {
    let count = || running.get_control(module, "request_count").unwrap();
    let before = count().as_number().unwrap();
    (running.set_control(module, "trigger", ControlValue::Bool(true))).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while count().as_number().unwrap() <= before {
        let status = text(running.get_control(module, "status").unwrap());
        assert_ne!(
            status,
            "error",
            "{:?}",
            running.get_control(module, "last_error")
        );
        assert!(Instant::now() < deadline, "the agent made no request");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Triggers one request of `module` and returns the packet it sent, as
/// the `test:echo` backend's history records it.
fn sent(running: &RunningInvention, module: &str) -> Value {
    request(running, module);
    let history = text(running.get_control(module, "history").unwrap());
    let history: Value = serde_json::from_str(&history).unwrap();
    history.as_array().unwrap().last().unwrap()["request"].clone()
}

fn echo(config: Value) -> Value {
    let mut config = config;
    config["backend"] = json!("test:echo");
    config
}

fn resolved(packs: &std::path::Path, reference: &str) -> String {
    crate::pkg::resolve_agent(reference, packs)
        .unwrap()
        .system_prompt
}

#[test]
fn a_request_sends_the_resolved_prompt_with_the_task() {
    with_pulse_agent(|packs| {
        let expected = resolved(packs, REFERENCE);
        let config = json!({ "definition": REFERENCE, "prompt": "Play a steady pulse." });
        let (running, _) = start(echo(config));
        let request = sent(&running, "improvise");
        assert_eq!(request["system"], expected.as_str());
        assert_eq!(request["task"], "Play a steady pulse.");
        running.stop();
        // An empty inline prompt beside the definition sends the resolved
        // one too.
        let (running, _) = start(echo(
            json!({ "definition": REFERENCE, "system_prompt": "" }),
        ));
        assert_eq!(sent(&running, "improvise")["system"], expected.as_str());
        running.stop();
    });
}

#[test]
fn a_requirement_picks_the_installed_version_it_admits() {
    with_pulse_agent(|packs| {
        let newer = json!({ "system_prompt": "The newer pulse." }).to_string();
        let skills = json!({ "skills": [] });
        install(
            packs,
            (PULSE_AGENT, "0.2.0", "agent"),
            skills,
            &[("agent.json", &newer)],
        );
        let older = format!("{}@=0.1.0", PULSE_AGENT);
        let (running, _) = start(echo(json!({ "definition": older })));
        assert_eq!(
            sent(&running, "improvise")["system"],
            resolved(packs, &older).as_str()
        );
        running.stop();
        let latest = format!("{}@>=0.1.0", PULSE_AGENT);
        let (running, _) = start(echo(json!({ "definition": latest })));
        assert_eq!(sent(&running, "improvise")["system"], "The newer pulse.");
        running.stop();
    });
}

#[test]
fn the_definition_resolves_once_as_the_piece_builds() {
    let packs = tempfile::tempdir().unwrap();
    install_pulse_agent(packs.path());
    let expected = resolved(packs.path(), REFERENCE);
    let mut built = None;
    with_packs_dir(packs.path(), || {
        let config = echo(json!({ "definition": REFERENCE }));
        built = Some(InventionBuilder::new(48_000).build(piece(config)).unwrap());
    });
    let (runtime, _) = built.unwrap();
    // The packages are gone before the piece starts or the agent asks.
    drop(packs);
    let running = runtime
        .start_with_backend(NullBackend::new(48_000))
        .unwrap();
    assert_eq!(sent(&running, "improvise")["system"], expected.as_str());
    running.stop();
}

#[test]
fn an_inline_prompt_needs_no_packages() {
    let missing = tempfile::tempdir().unwrap().path().join("absent");
    with_packs_dir(&missing, || {
        let (running, _) = start(echo(json!({ "system_prompt": "You answer." })));
        assert_eq!(sent(&running, "improvise")["system"], "You answer.");
        running.stop();
    });
}

#[test]
fn default_tools_do_not_limit_what_a_response_applies() {
    with_pulse_agent(|packs| {
        let toolful = json!({
            "system_prompt": "You use tools.",
            "default_tools": ["no.such.tool", "fugue.not_a_tool"],
        });
        install(
            packs,
            ("fugue.demo.toolful-agent", "1.0.0", "agent"),
            json!({}),
            &[("agent.json", &toolful.to_string())],
        );
        let document = json!({
            "version": "1.0.0",
            "modules": [
                {
                    "id": "bass_seq",
                    "type": "step_sequencer",
                    "config": { "root_note": 36, "step_count": 4, "pattern": [{ "note": 0 }, { "note": 7 }] },
                },
                {
                    "id": "improvise",
                    "type": "agent",
                    "config": {
                        "backend": "test:response",
                        "definition": "fugue.demo.toolful-agent@^1",
                        "prompt": "Vary the motif.",
                        "response": { "format": "json", "kind": "pattern_variation" },
                        "test_response": {
                            "kind": "pattern_variation",
                            "summary": "variation",
                            "payload": { "pattern": [{ "note": 0 }, { "note": 3 }, { "note": 5 }] },
                            "confidence": 1.0,
                            "warnings": [],
                        },
                        "apply": [{
                            "from": "$.payload.pattern",
                            "to": "bass_seq",
                            "control": "pattern",
                            "type": "json_string",
                        }],
                    },
                },
                { "id": "dac", "type": "dac" }
            ],
            "connections": []
        });
        let running = play(Invention::from_json(&document.to_string()).unwrap());
        request(&running, "improvise");
        let pattern = text(running.get_control("bass_seq", "pattern").unwrap());
        let pattern: Value = serde_json::from_str(&pattern).unwrap();
        assert_eq!(pattern.as_array().unwrap().len(), 3, "{pattern}");
        running.stop();
    });
}
