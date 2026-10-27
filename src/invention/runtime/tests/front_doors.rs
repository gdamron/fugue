//! Every front door's write to a declared control reaches its module as a
//! request the audio thread applies (or, for a schedule, in a publication),
//! never through the module's setter. A type that declares its controls is
//! checked control by control; only the controls listed here, which hold
//! no audio-thread state, are written directly.

use super::*;
use crate::control_request::Source;
use crate::invention::declared::WriteOptions;
use crate::invention::manual_backend::{start_manual, Pump};
use crate::module_config::tests::registry::{base_config, UNBUILDABLE};
use crate::ControlWriteIntent;

/// Controls of a declaring type written directly: text the module reads
/// on a control thread, never on the audio thread.
const DIRECT: &[(&str, &str, &str)] = &[
    ("agent", "prompt", "read by the agent's worker thread"),
    (
        "agent",
        "system_prompt",
        "read by the agent's worker thread",
    ),
    ("agent", "backend", "read by the agent's worker thread"),
    ("code", "status", "the script runtime's own report"),
    ("code", "last_error", "the script runtime's own report"),
];

/// A live session playing one `type_id` module.
fn alone(type_id: &str) -> (RunningInvention, Pump) {
    let mut config = base_config(type_id);
    if type_id == "sample_slicer" {
        // Its controls exist only in elastic mode.
        config["mode"] = serde_json::json!("elastic");
    }
    let document = serde_json::json!({
        "version": "1.0.0",
        "modules": [{ "id": "m", "type": type_id, "config": config }],
        "connections": []
    });
    let invention = Invention::from_json(&document.to_string()).unwrap();
    let built = InventionBuilder::new(48_000).build(invention);
    let runtime = built.unwrap_or_else(|error| panic!("'{type_id}' did not build: {error}"));
    start_manual(runtime.0)
}

#[test]
fn a_declaring_types_front_door_writes_are_requests_or_publications() {
    let registry = crate::ModuleRegistry::default();
    let (mut checked, mut direct) = (Vec::new(), Vec::new());
    for type_id in registry.types() {
        let skipped = registry.is_sink(type_id) || UNBUILDABLE.iter().any(|(id, _)| *id == type_id);
        if skipped {
            continue;
        }
        let (running, pump) = alone(type_id);
        let Ok(controls) = running.list_controls("m") else {
            continue;
        };
        let snapshot = running.snapshot();
        for meta in controls {
            let key = meta.key.as_str();
            let listed = DIRECT
                .iter()
                .any(|(id, at, _)| (*id, *at) == (type_id, key));
            // Its own value, written back as a gesture would.
            let Ok(value) = running.get_control("m", key) else {
                continue;
            };
            let (intent, options) = (
                ControlWriteIntent::Perform,
                WriteOptions::performed(Source::User),
            );
            let Ok(ack) = snapshot.submit_control("m", key, value, intent, options) else {
                continue;
            };
            pump.block();
            let requested = ack.request.is_some() || ack.published.is_some();
            if requested == listed {
                direct.push(format!("{type_id}.{key} (listed {listed})"));
            }
        }
        checked.push(type_id);
        running.stop();
    }
    assert!(
        direct.is_empty(),
        "written other than as listed: {direct:?}"
    );
    for expected in ["oscillator", "clock", "step_sequencer", "code", "agent"] {
        assert!(checked.contains(&expected), "'{expected}' not checked");
    }
}

/// The files that may write a surface directly (`surface.set_control`) or
/// skip the front door's coercion and announcement
/// (`set_control_transient`), and why. Every other write goes through
/// `RuntimeSnapshot::set_control_with_intent` or `submit_control`.
const WRITES_A_SURFACE: &[(&str, &str)] = &[
    (
        "invention/development.rs",
        "a development configures its inner modules as it builds",
    ),
    (
        "invention/edits/apply/commit.rs",
        "a batch's writes: carried, or to modules it built and has not yet played",
    ),
    (
        "invention/edits/apply/facts.rs",
        "validation, on a copy of a surface that never plays",
    ),
    (
        "invention/reload/commit.rs",
        "authored values carried into a rebuilt graph",
    ),
    (
        "invention/render/code_modules.rs",
        "a code module's own status report",
    ),
    (
        "invention/render/replay/edits.rs",
        "a replay rebuilding modules as the batch made them, offline",
    ),
    (
        "modules/cell_sequencer/factory.rs",
        "configuring the module as it builds",
    ),
    (
        "modules/step_sequencer/factory.rs",
        "configuring the module as it builds",
    ),
    (
        "scripting/native/js_interop.rs",
        "the script runtime's own status report",
    ),
];

/// Every `.rs` file under `dir`, but tests.
fn sources(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" && name != "test_support" {
                sources(&path, found);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            found.push(path);
        }
    }
}

#[test]
fn only_the_listed_files_write_a_surface_past_the_front_door() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    let mut writers = Vec::new();
    for path in files {
        // Line endings as checked out (CRLF on Windows) read as LF.
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n");
        // Up to its inline tests, if it has any.
        let code = text.split("#[cfg(test)]\nmod tests {").next().unwrap();
        let direct = ["surface.set_control(", ".set_control_transient("];
        if direct.iter().any(|call| code.contains(call)) {
            let relative = path.strip_prefix(&root).unwrap();
            writers.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    writers.sort();
    let mut listed: Vec<_> = WRITES_A_SURFACE
        .iter()
        .map(|(file, _)| file.to_string())
        .collect();
    listed.sort();
    assert_eq!(
        writers, listed,
        "a surface written directly outside the list"
    );
}
