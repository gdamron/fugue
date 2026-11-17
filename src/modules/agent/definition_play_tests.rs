//! A running agent with a definition, as its requests and the piece hear
//! it: the prompt it sends, the version it resolved, and what a reload or a
//! refused change leaves playing.

use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{install, install_pulse_agent, piece, start, text, with_pulse_agent};
use super::{PERSONA, PULSE_AGENT, REFERENCE};
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

fn resolved(packs: &Path, reference: &str) -> String {
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
        let system = sent(&running, "improvise")["system"].clone();
        assert_eq!(system, resolved(packs, &older).as_str());
        // The pinned older version's persona, not the newer one's.
        assert!(system.as_str().unwrap().starts_with(PERSONA), "{system}");
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
fn a_refused_reload_or_edit_leaves_the_document_and_the_prompt_sent() {
    with_pulse_agent(|packs| {
        let expected = resolved(packs, REFERENCE);
        let (mut running, _) = start(echo(json!({ "definition": REFERENCE })));
        let before = serde_json::to_value(running.document().unwrap()).unwrap();
        // One names a package that is not installed; the other, one whose
        // skill is not.
        let broken = "fugue.demo.broken-agent";
        let skills = json!({ "skills": ["fugue.demo.absent-skill@^1"] });
        let persona = json!({ "system_prompt": "You break." }).to_string();
        install(
            packs,
            (broken, "1.0.0", "agent"),
            skills,
            &[("agent.json", &persona)],
        );
        for package in ["fugue.demo.absent", broken] {
            let config = echo(json!({ "definition": format!("{package}@^1") }));
            let error = running
                .reload(piece(config.clone()))
                .unwrap_err()
                .to_string();
            assert!(error.contains("module 'improvise'"), "{error}");
            assert!(error.contains(package), "{error}");
            let edit = crate::rpc::StructuralEdit::AddModule {
                id: "second".into(),
                module_type: "agent".into(),
                config,
                name: None,
            };
            let error = format!("{:?}", running.apply_edits(&[edit]).unwrap_err());
            assert!(error.contains("second"), "{error}");
            assert!(error.contains(package), "{error}");
            // Neither changed the document: no module was added.
            let after = serde_json::to_value(running.document().unwrap()).unwrap();
            assert_eq!(after, before);
        }
        assert_eq!(sent(&running, "improvise")["system"], expected.as_str());
        running.stop();
    });
}

#[test]
fn a_reload_to_another_definition_shows_only_its_parts() {
    with_pulse_agent(|packs| {
        let plain = json!({ "system_prompt": "You are plain." }).to_string();
        install(
            packs,
            ("fugue.demo.plain-agent", "1.0.0", "agent"),
            json!({}),
            &[("agent.json", &plain)],
        );
        let (mut running, _) = start(echo(json!({ "definition": REFERENCE })));
        let other = echo(json!({ "definition": "fugue.demo.plain-agent@^1" }));
        running.reload(piece(other)).unwrap();
        let summary = text(
            running
                .get_control("improvise", "resolved_definition")
                .unwrap(),
        );
        let summary: Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(summary["id"], "fugue.demo.plain-agent");
        assert_eq!(
            summary["parts"],
            json!([{
                "source": "persona",
                "package": "fugue.demo.plain-agent",
                "version": "1.0.0",
                "chars": "You are plain.".len(),
            }])
        );
        assert_eq!(sent(&running, "improvise")["system"], "You are plain.");
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

#[test]
fn the_example_loads_with_the_demo_agent_installed() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/agent_definition_pulse.json");
    let invention = Invention::from_file(path.to_str().unwrap()).unwrap();
    let spec = invention
        .modules
        .iter()
        .find(|spec| spec.module_type == "agent")
        .unwrap();
    assert_eq!(spec.config["definition"], REFERENCE);
    with_pulse_agent(|_| {
        let mut engine = crate::RenderEngine::new(48_000);
        engine.stub_agent_backends(true);
        engine.load_invention(invention.clone()).unwrap();
        // Two seconds of it, with the agent idle, still sound.
        let mut block = vec![0.0f32; 1024];
        let mut peak = 0.0f32;
        for _ in 0..(2 * 48_000usize).div_ceil(512) {
            engine.render_interleaved(&mut block).unwrap();
            peak = block
                .iter()
                .fold(peak, |peak, sample| peak.max(sample.abs()));
        }
        assert!(peak > 0.01, "peak {peak}");
        let prompt = text(engine.get_control(&spec.id, "system_prompt").unwrap());
        assert!(prompt.starts_with(PERSONA), "{prompt}");
    });
    // Without the package, the load is refused naming it.
    let empty = tempfile::tempdir().unwrap();
    with_packs_dir(empty.path(), || {
        let error = InventionBuilder::new(48_000)
            .build(invention)
            .err()
            .unwrap();
        assert!(error.to_string().contains(PULSE_AGENT), "{error}");
    });
}
