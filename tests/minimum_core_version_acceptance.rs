//! Acceptance tests for a document's `minimum_core_version`.
//!
//! A document (an invention or a development) may declare the oldest core
//! that understands it, as `"minimum_core_version": "YYYY.M.P"`. A core older
//! than that refuses the document when it is parsed, naming the required and
//! the running version, and nothing in a running session changes. A minimum
//! at or below the running core, or none at all, loads exactly as before.
//!
//! The refusal is exercised through every way a document enters the core:
//! the parsers, the offline engine, and the commands a daemon deserializes
//! for a live load or reload.

use std::sync::{Arc, Mutex};

use fugue::rpc::{EditTiming, RpcCommand, RpcRequest};
use fugue::{
    AudioBackend, ControlValue, Invention, InventionBuilder, RenderEngine, RunningInvention,
};
use serde_json::{json, Value};

mod minimum_core_version_support;
use minimum_core_version_support::*;

const RATE: u32 = 48_000;
const BLOCK: usize = 64;

// -- entry points --------------------------------------------------------------------

type Entry = (&'static str, fn(&Value) -> Result<(), String>);

fn text<E: std::fmt::Display>(result: Result<impl Sized, E>) -> Result<(), String> {
    result.map(|_| ()).map_err(|e| e.to_string())
}

fn from_json(doc: &Value) -> Result<(), String> {
    text(Invention::from_json(&doc.to_string()))
}

fn from_file(doc: &Value) -> Result<(), String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("piece.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    text(Invention::from_file(path.to_str().unwrap()))
}

fn from_value(doc: &Value) -> Result<(), String> {
    text(serde_json::from_value::<Invention>(doc.clone()))
}

fn offline_engine(doc: &Value) -> Result<(), String> {
    text(RenderEngine::new(RATE).load_json(&doc.to_string()))
}

/// A load or reload command as a client sends it, carrying `doc` verbatim.
fn command(load: bool, doc: &Value) -> Value {
    let invention = Box::new(parse(&plain()));
    let command = if load {
        RpcCommand::LoadInvention {
            invention,
            source_path: None,
            read_source: false,
            frozen: false,
            stop_on_end: false,
            end_source: None,
        }
    } else {
        RpcCommand::ReloadInvention {
            invention,
            source_path: None,
            read_source: false,
            frozen: false,
            timing: EditTiming::default(),
        }
    };
    let mut wire = serde_json::to_value(command).unwrap();
    wire["invention"] = doc.clone();
    wire
}

fn rpc_load(doc: &Value) -> Result<(), String> {
    text(serde_json::from_value::<RpcCommand>(command(true, doc)))
}

fn rpc_reload(doc: &Value) -> Result<(), String> {
    text(serde_json::from_value::<RpcCommand>(command(false, doc)))
}

/// The same commands inside the request envelope a daemon reads.
fn request(load: bool, doc: &Value) -> Value {
    let mut wire = serde_json::to_value(RpcRequest::new(RpcCommand::UnloadInvention)).unwrap();
    let inner = command(load, doc);
    for (key, value) in inner.as_object().unwrap() {
        wire[key] = value.clone();
    }
    wire
}

fn request_load(doc: &Value) -> Result<(), String> {
    text(serde_json::from_value::<RpcRequest>(request(true, doc)))
}

fn request_reload(doc: &Value) -> Result<(), String> {
    text(serde_json::from_value::<RpcRequest>(request(false, doc)))
}

const ENTRIES: [Entry; 8] = [
    ("Invention::from_json", from_json),
    ("Invention::from_file", from_file),
    ("serde Invention", from_value),
    ("RenderEngine::load_json", offline_engine),
    ("RpcCommand::LoadInvention", rpc_load),
    ("RpcCommand::ReloadInvention", rpc_reload),
    ("RpcRequest load_invention", request_load),
    ("RpcRequest reload_invention", request_reload),
];

fn assert_refused_naming(result: Result<(), String>, required: &str, label: &str) {
    let error = result.expect_err(&format!("{label}: a minimum of {required} must be refused"));
    assert!(
        error.contains(required) && error.contains(RUNNING),
        "{label}: the error must name the required ({required}) and the running ({RUNNING}) \
         version, got: {error}"
    );
}

// -- 1. a newer minimum is refused --------------------------------------------------

#[test]
fn a_document_needing_a_newer_core_is_refused_naming_both_versions() {
    let doc = declaring(plain(), FAR_FUTURE);
    for (label, enter) in ENTRIES {
        assert_refused_naming(enter(&doc), FAR_FUTURE, label);
    }
}

#[test]
fn a_development_document_needing_a_newer_core_is_refused() {
    let doc = declaring(kit_definition(false), FAR_FUTURE);
    for (label, enter) in ENTRIES {
        assert_refused_naming(enter(&doc), FAR_FUTURE, label);
    }
}

#[test]
fn an_inline_development_definition_needing_a_newer_core_is_refused() {
    let doc = kit_document(declaring(kit_definition(false), FAR_FUTURE));
    for (label, enter) in ENTRIES {
        assert_refused_naming(enter(&doc), FAR_FUTURE, label);
    }
}

#[test]
fn the_next_release_of_every_component_is_too_new() {
    let (year, month, patch) = running();
    let next = [
        format!("{year}.{month}.{}", patch + 1),
        format!("{year}.{}.0", month + 1),
        format!("{}.1.0", year + 1),
        // Longer than the running month, yet numerically larger.
        format!("{year}.{}.0", month * 10 + 1),
    ];
    for minimum in next {
        let doc = declaring(plain(), minimum.as_str());
        for (label, enter) in ENTRIES {
            assert_refused_naming(enter(&doc), &minimum, label);
        }
    }
}

/// `doc` with its development named by the file `kit.json`, not inline.
fn by_path(mut doc: Value) -> Value {
    doc["developments"] = json!([ { "name": "kit", "path": "kit.json" } ]);
    doc
}

#[test]
fn a_nested_path_development_needing_a_newer_core_is_refused_when_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let development = declaring(kit_definition(false), FAR_FUTURE);
    std::fs::write(dir.path().join("kit.json"), development.to_string()).unwrap();
    let parent = by_path(kit_document(Value::Null));
    let path = dir.path().join("piece.json");
    std::fs::write(&path, parent.to_string()).unwrap();

    // The parent declares nothing, so reading it may succeed; the nested
    // document is refused no later than building the graph.
    let outcome = Invention::from_file(path.to_str().unwrap())
        .map_err(|e| e.to_string())
        .and_then(|invention| text(InventionBuilder::new(RATE).build(invention)));
    assert_refused_naming(outcome, FAR_FUTURE, "a development loaded by path");
}

#[test]
fn a_nested_path_development_within_the_running_core_loads() {
    let dir = tempfile::tempdir().unwrap();
    let development = declaring(kit_definition(false), RUNNING);
    std::fs::write(dir.path().join("kit.json"), development.to_string()).unwrap();
    let parent = by_path(kit_document(Value::Null));
    let path = dir.path().join("piece.json");
    std::fs::write(&path, parent.to_string()).unwrap();

    let invention = Invention::from_file(path.to_str().unwrap()).expect("parses");
    InventionBuilder::new(RATE)
        .build(invention)
        .unwrap_or_else(|e| panic!("builds: {e}"));
}

#[test]
fn a_malformed_minimum_is_refused() {
    let malformed: [Value; 7] = [
        json!("soon"),
        json!("2026.11"),
        json!("2026.11.0.1"),
        json!("v2026.11.0"),
        json!("2026.11.x"),
        json!(""),
        json!(202611),
    ];
    for minimum in malformed {
        let doc = declaring(plain(), minimum.clone());
        for (label, enter) in ENTRIES {
            assert!(
                enter(&doc).is_err(),
                "{label}: the malformed minimum {minimum} must be refused"
            );
        }
    }
}

// -- 2. a minimum at or below the running core loads ----------------------------------

/// Each component compared as a number, not as text: 2026.9.0 is older than
/// 2026.10.0, and 2026.100.0 is newer than 2026.11.0.
#[test]
fn versions_compare_numerically_per_component() {
    let declared = [
        "2026.1.0",
        "2026.8.0",
        "2026.9.0",
        "2026.10.0",
        "2026.11.0",
        "2026.12.0",
        "2026.100.0",
        "2027.1.0",
        RUNNING,
    ];
    for minimum in declared {
        let doc = declaring(plain(), minimum);
        let loads = version(minimum).unwrap() <= running();
        for (label, enter) in ENTRIES {
            match (loads, enter(&doc)) {
                (true, Ok(())) | (false, Err(_)) => {}
                (true, Err(e)) => panic!("{label}: {minimum} <= {RUNNING} must load: {e}"),
                (false, Ok(())) => panic!("{label}: {minimum} > {RUNNING} must be refused"),
            }
        }
    }
}

#[test]
fn a_minimum_equal_to_the_running_core_loads() {
    for doc in [
        declaring(plain(), RUNNING),
        kit_document(declaring(kit_definition(true), RUNNING)),
    ] {
        for (label, enter) in ENTRIES {
            enter(&doc).unwrap_or_else(|e| panic!("{label}: loads at the running core: {e}"));
        }
    }
}

fn render(doc: &Value) -> Vec<f32> {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(&doc.to_string()).expect("loads");
    engine.set_block_size(BLOCK);
    let mut buffer = vec![0.0f32; 2 * 16 * BLOCK];
    engine.render_interleaved(&mut buffer).expect("renders");
    buffer
}

#[test]
fn a_loadable_minimum_changes_nothing_else_about_the_document() {
    let cases = [
        plain(),
        kit_document(kit_definition(true)),
        kit_document(kit_definition(false)),
    ];
    for bare in cases {
        for minimum in ["2026.1.0", RUNNING] {
            let mut declared = declaring(bare.clone(), minimum);
            if let Some(spec) = (declared.get_mut("developments")).and_then(|d| d.get_mut(0)) {
                spec["definition"][FIELD] = json!(minimum);
            }
            assert_eq!(
                normalized(&declared),
                normalized(&bare),
                "the parsed document differs beyond the field (minimum {minimum})"
            );
            let (with, without) = (render(&declared), render(&bare));
            assert!(
                without.iter().any(|s| s.abs() > 1e-3),
                "the document under test must make sound"
            );
            assert_eq!(
                with, without,
                "minimum {minimum} changed the rendered samples"
            );
        }
    }
}

// -- 3. a document without the field loads as today ------------------------------------

#[test]
fn a_document_without_the_field_loads() {
    let cases = [
        plain(),
        kit_definition(false),
        kit_document(kit_definition(false)),
        kit_document(kit_definition(true)),
    ];
    for doc in cases {
        for (label, enter) in ENTRIES {
            enter(&doc).unwrap_or_else(|e| panic!("{label}: loads without the field: {e}"));
        }
    }
}

// -- nothing changes in the running session ---------------------------------------------

type Render = Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>;

struct Capture(Arc<Mutex<Option<Render>>>);

impl AudioBackend for Capture {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn start(&mut self, render: Render) -> Result<(), Box<dyn std::error::Error>> {
        *self.0.lock().unwrap() = Some(render);
        Ok(())
    }
    fn stop(&mut self) {
        self.0.lock().unwrap().take();
    }
}

struct Session {
    running: RunningInvention,
    slot: Arc<Mutex<Option<Render>>>,
}

impl Session {
    fn start(doc: &Value) -> Session {
        let (runtime, _) = InventionBuilder::new(RATE)
            .build(parse(doc))
            .expect("builds");
        let slot = Arc::new(Mutex::new(None));
        let running = runtime
            .start_with_backend(Capture(slot.clone()))
            .expect("a capturing backend starts");
        Session { running, slot }
    }

    fn render(&self, blocks: usize) -> Vec<f32> {
        let mut slot = self.slot.lock().unwrap();
        let render = slot.as_mut().expect("backend started");
        let (mut left, mut right) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
        let mut out = Vec::new();
        for _ in 0..blocks {
            render(&mut left, &mut right);
            out.extend_from_slice(&left);
        }
        out
    }

    fn frequency(&self) -> f32 {
        self.render(4);
        (self.running.get_control("osc", "frequency").unwrap())
            .as_number()
            .unwrap()
    }

    fn document(&self) -> String {
        self.running
            .document()
            .expect("retained")
            .to_json()
            .unwrap()
    }
}

#[test]
fn a_refused_load_or_reload_leaves_the_running_session_untouched() {
    let mut session = Session::start(&plain());
    session
        .running
        .set_control("osc", "frequency", ControlValue::Number(330.0))
        .unwrap();
    assert_eq!(session.frequency(), 330.0);
    let document_before = session.document();

    // What a daemon does with a too-new load or reload: it cannot even read
    // the command, so there is no document to apply.
    let too_new = declaring(plain(), FAR_FUTURE);
    for (label, enter) in [
        (
            "load_invention",
            rpc_load as fn(&Value) -> Result<(), String>,
        ),
        ("reload_invention", rpc_reload),
        ("request load_invention", request_load),
        ("request reload_invention", request_reload),
    ] {
        assert_refused_naming(enter(&too_new), FAR_FUTURE, label);
    }
    assert_refused_naming(from_json(&too_new), FAR_FUTURE, "from_json");

    assert_eq!(session.document(), document_before, "the document changed");
    assert_eq!(session.frequency(), 330.0, "a control changed");
    let heard = session.render(16);
    assert!(
        heard.iter().any(|s| s.abs() > 1e-3),
        "the session stopped sounding"
    );

    // The same command with a minimum the core meets applies as usual.
    let mut next = declaring(plain(), RUNNING);
    next["modules"][0]["config"]["frequency"] = json!(440.0);
    let wire = command(false, &next);
    let Ok(RpcCommand::ReloadInvention { invention, .. }) = serde_json::from_value(wire) else {
        panic!("a reload within the running core is read");
    };
    session.running.reload(*invention).expect("reloads");
    assert_eq!(session.frequency(), 440.0);
}
