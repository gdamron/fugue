//! Acceptance tests: a piece names its modules, and module types declare
//! labels and preferred widgets.
//!
//! Written from the issue's acceptance criteria and the public interfaces
//! alone. Each test names the criterion it checks:
//!
//! - NAME-FORMAT: "A module in an invention may carry an optional name. Load,
//!   get, save and the RPC and MCP schemas carry it."
//! - NAME-EDIT: "A document-only edit sets or clears a module's name. It
//!   changes the revision and rebuilds no module, and audio continues
//!   without a glitch."
//! - NAME-COMPAT: "An invention without names loads, plays and saves as
//!   before. A core without this change ignores and drops names, as it does
//!   the layout, so minimum_core_version doesn't rise."
//! - LABELS: "A module type can declare a label for itself, for each control,
//!   for each value of a choice control, and for each input and output.
//!   Module descriptions and listings report them."
//! - WIDGET: "A module type can declare the widget a control prefers (knob,
//!   slider, stepper, segmented, menu), reported with the control. Nothing is
//!   reported when a type declares none."
//! - TIDY: "A label is listed only when it differs from what tidying the key
//!   gives."
//! - FIRST-PARTY: "At least the clock, the filter and the oscillator declare
//!   labels, as the pattern for the rest."
//! - DESCRIBE: "describe_module output lets an agent see a module's name and
//!   labels and still address it by id."

mod support;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use fugue::rpc::{
    ApplyEditsReport, DescribeModuleQuery, ModuleTypeQuery, RpcCommand, RpcRequest, StructuralEdit,
    TypeDetail,
};
use fugue::{AudioBackend, ControlMeta, Invention, InventionBuilder, RunningInvention};
use serde_json::{json, Value};

const RATE: u32 = 48_000;

// -- documents ------------------------------------------------------------------------

fn wire(from: &str, from_port: &str, to: &str, to_port: &str) -> Value {
    json!({ "from": from, "from_port": from_port, "to": to, "to_port": to_port })
}

/// An oscillator through a filter into the dac; nothing is named.
fn plain() -> Value {
    json!({
        "version": "1.0.0",
        "title": "names",
        "modules": [
            { "id": "osc", "type": "oscillator",
              "config": { "waveform": "sine", "frequency": 220.0 } },
            { "id": "lpf", "type": "filter",
              "config": { "filter_type": "lowpass", "cutoff": 9000.0, "resonance": 0.0 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } },
        ],
        "connections": [
            wire("osc", "audio", "lpf", "audio"),
            wire("lpf", "audio", "dac", "audio"),
        ],
    })
}

/// `plain()` with the filter and the oscillator named.
fn named() -> Value {
    let mut doc = plain();
    doc["modules"][0]["name"] = json!("Drone");
    doc["modules"][1]["name"] = json!("Lead filter");
    doc
}

fn parse(doc: &Value) -> Invention {
    Invention::from_json(&doc.to_string()).unwrap_or_else(|e| panic!("parses: {e}\n{doc:#}"))
}

fn saved(invention: &Invention) -> Value {
    serde_json::from_str(&invention.to_json().expect("serializes")).expect("is JSON")
}

fn module<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["modules"]
        .as_array()
        .expect("modules is a list")
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("no module {id} in {doc:#}"))
}

fn name_of(doc: &Value, id: &str) -> Option<String> {
    module(doc, id)
        .get("name")
        .map(|n| n.as_str().expect("a name is a string").to_string())
}

// -- a running session ------------------------------------------------------------------

/// Renders on a thread, as a device would, recording the left channel.
struct Recorder {
    out: Arc<Mutex<Vec<f32>>>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Recorder {
    fn new(out: Arc<Mutex<Vec<f32>>>) -> Self {
        Recorder {
            out,
            running: Arc::new(AtomicBool::new(false)),
            worker: None,
        }
    }
}

impl AudioBackend for Recorder {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn start(
        &mut self,
        mut render: Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let out = self.out.clone();
        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);
        self.worker = Some(thread::spawn(move || {
            let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
            while running.load(Ordering::SeqCst) {
                render(&mut left, &mut right);
                out.lock().unwrap().extend_from_slice(&left);
                thread::sleep(Duration::from_millis(1));
            }
        }));
        Ok(())
    }
    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Session {
    running: RunningInvention,
    out: Arc<Mutex<Vec<f32>>>,
}

impl Session {
    fn start(doc: &Value) -> Session {
        let (runtime, _) = InventionBuilder::new(RATE)
            .build(parse(doc))
            .expect("builds");
        let out = Arc::new(Mutex::new(Vec::new()));
        let running = runtime
            .start_with_backend(Recorder::new(out.clone()))
            .expect("starts");
        Session { running, out }
    }

    fn document(&self) -> Value {
        saved(&self.running.document().expect("a retained document"))
    }

    fn edit(&mut self, edits: Value) -> Result<ApplyEditsReport, String> {
        let edits: Vec<StructuralEdit> =
            serde_json::from_value(edits).map_err(|e| format!("parse: {e}"))?;
        self.running
            .apply_edits(&edits)
            .map_err(|e| format!("{e:?}"))
    }

    fn describe(&self, id: &str) -> Value {
        let query = DescribeModuleQuery {
            module_id: Some(id.to_string()),
            ..Default::default()
        };
        serde_json::to_value(self.running.describe_module(&query).expect("describes")).unwrap()
    }

    /// Blocks until `samples` more samples have been rendered.
    fn render_more(&self, samples: usize) {
        let target = self.out.lock().unwrap().len() + samples;
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.out.lock().unwrap().len() < target {
            assert!(Instant::now() < deadline, "audio stopped rendering");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn recorded(&self) -> Vec<f32> {
        self.out.lock().unwrap().clone()
    }
}

// =====================================================================================
// NAME-FORMAT
// =====================================================================================

/// NAME-FORMAT: load and save carry a module's name.
#[test]
fn a_module_name_survives_load_and_save() {
    let saved_doc = saved(&parse(&named()));
    assert_eq!(name_of(&saved_doc, "osc").as_deref(), Some("Drone"));
    assert_eq!(name_of(&saved_doc, "lpf").as_deref(), Some("Lead filter"));
    assert_eq!(name_of(&saved_doc, "dac"), None);
}

/// NAME-FORMAT: the same through a file on disk.
#[test]
fn a_module_name_survives_save_to_file_and_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("piece.json");
    parse(&named()).save_to_file(&path).unwrap();
    let on_disk: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(name_of(&on_disk, "lpf").as_deref(), Some("Lead filter"));
    let loaded = Invention::from_file(path.to_str().unwrap()).unwrap();
    assert_eq!(
        name_of(&saved(&loaded), "osc").as_deref(),
        Some("Drone"),
        "a name read back from a saved file"
    );
}

/// NAME-FORMAT: the serde value form (what an RPC load carries) keeps names.
#[test]
fn a_module_name_survives_the_serde_value_form() {
    let invention: Invention = serde_json::from_value(named()).unwrap();
    assert_eq!(
        name_of(&saved(&invention), "lpf").as_deref(),
        Some("Lead filter")
    );
    let again = serde_json::to_value(&invention).unwrap();
    assert_eq!(name_of(&again, "lpf").as_deref(), Some("Lead filter"));
}

/// NAME-FORMAT: names are kept verbatim, including spaces and non-ASCII text.
#[test]
fn names_are_kept_verbatim() {
    let mut doc = plain();
    doc["modules"][1]["name"] = json!("  Süß \"filter\" – 低域  ");
    assert_eq!(
        name_of(&saved(&parse(&doc)), "lpf").as_deref(),
        Some("  Süß \"filter\" – 低域  ")
    );
}

/// NAME-FORMAT: the running session's retained document (what get_invention
/// and save_invention read) carries the names it was loaded with.
#[test]
fn a_running_session_retains_names_for_get_and_save() {
    let session = Session::start(&named());
    assert_eq!(
        name_of(&session.document(), "lpf").as_deref(),
        Some("Lead filter")
    );
}

/// NAME-FORMAT: the invention JSON schema (which the RPC and MCP schemas are
/// built from) describes an optional `name` on a module.
#[cfg(feature = "rpc-schema")]
#[test]
fn the_invention_schema_describes_an_optional_module_name() {
    fn find(value: &Value, found: &mut Vec<Value>) {
        match value {
            Value::Object(map) => {
                if let Some(props) = map.get("properties").and_then(Value::as_object) {
                    if ["id", "type", "config"]
                        .iter()
                        .all(|k| props.contains_key(*k))
                    {
                        found.push(value.clone());
                    }
                }
                map.values().for_each(|v| find(v, found));
            }
            Value::Array(items) => items.iter().for_each(|v| find(v, found)),
            _ => {}
        }
    }
    let schema = serde_json::to_value(schemars::schema_for!(Invention)).unwrap();
    let mut specs = Vec::new();
    find(&schema, &mut specs);
    assert!(!specs.is_empty(), "no module spec in the schema");
    for spec in specs {
        assert!(spec["properties"].get("name").is_some(), "{spec:#}");
        let required = spec["required"].as_array().cloned().unwrap_or_default();
        assert!(!required.contains(&json!("name")), "a name is optional");
    }
}

/// NAME-FORMAT: the apply_edits wire schema describes `name_modules`.
#[cfg(feature = "rpc-schema")]
#[test]
fn the_edit_schema_describes_name_modules_and_add_module_name() {
    let schema = serde_json::to_string(&schemars::schema_for!(StructuralEdit)).unwrap();
    assert!(schema.contains("name_modules"), "{schema}");
    assert!(schema.contains("\"names\""), "{schema}");
}

// =====================================================================================
// NAME-COMPAT
// =====================================================================================

/// NAME-COMPAT: an invention without names saves with no `name` key at all
/// (not null, not an empty string).
#[test]
fn unnamed_modules_save_without_a_name_key() {
    let doc = saved(&parse(&plain()));
    for m in doc["modules"].as_array().unwrap() {
        assert!(m.get("name").is_none(), "unexpected name key: {m}");
    }
    // Partly named: only the named module carries the key.
    let mixed = saved(&parse(&named()));
    assert!(module(&mixed, "dac").get("name").is_none());
}

/// NAME-COMPAT: a document without names loads, builds, plays and re-saves
/// to the same document as before (a second round trip changes nothing).
#[test]
fn an_unnamed_invention_loads_plays_and_saves_as_before() {
    let first = saved(&parse(&plain()));
    let second = saved(&parse(&first));
    assert_eq!(first, second);

    let session = Session::start(&plain());
    session.render_more(4_800);
    let out = session.recorded();
    assert!(
        out.iter().any(|s| s.abs() > 0.05),
        "an unnamed piece must still sound"
    );
    assert_eq!(session.document(), first);
    session.running.stop();
}

/// NAME-COMPAT: naming modules does not raise `minimum_core_version`: the
/// saved document declares the same minimum as the unnamed one.
#[test]
fn names_do_not_raise_minimum_core_version() {
    let unnamed = saved(&parse(&plain()));
    let with_names = saved(&parse(&named()));
    assert_eq!(
        unnamed.get("minimum_core_version"),
        with_names.get("minimum_core_version"),
        "naming a module changed the document's minimum core"
    );
    // A declared minimum is kept exactly, with or without names.
    let mut declared = named();
    declared["minimum_core_version"] = json!("2026.1.0");
    let again = saved(&parse(&declared));
    let mut declared_plain = plain();
    declared_plain["minimum_core_version"] = json!("2026.1.0");
    assert_eq!(
        again.get("minimum_core_version"),
        saved(&parse(&declared_plain)).get("minimum_core_version")
    );
}

/// NAME-COMPAT: a name is optional metadata: dropping every name from the
/// saved document leaves a document that loads and builds (what an older core
/// sees).
#[test]
fn a_document_stripped_of_names_still_loads() {
    let mut doc = saved(&parse(&named()));
    for m in doc["modules"].as_array_mut().unwrap() {
        m.as_object_mut().unwrap().remove("name");
    }
    assert_eq!(doc, saved(&parse(&plain())));
    InventionBuilder::new(RATE)
        .build(parse(&doc))
        .expect("builds");
}

// =====================================================================================
// NAME-EDIT
// =====================================================================================

/// NAME-EDIT: name_modules sets a name, in the retained document and in what
/// describe_module reports; the module stays addressable by id (DESCRIBE).
#[test]
fn name_modules_sets_a_name_visible_in_the_document_and_describe_module() {
    let mut session = Session::start(&plain());
    assert!(session
        .describe("lpf")
        .get("name")
        .map_or(true, Value::is_null));
    session
        .edit(json!([{ "op": "name_modules", "names": { "lpf": "Lead filter" } }]))
        .expect("the edit commits");
    assert_eq!(
        name_of(&session.document(), "lpf").as_deref(),
        Some("Lead filter")
    );
    let description = session.describe("lpf");
    assert_eq!(description["name"], "Lead filter");
    assert_eq!(description["module_id"], "lpf", "still addressable by id");
    // The unnamed neighbour is untouched.
    assert_eq!(name_of(&session.document(), "osc"), None);
    session.running.stop();
}

/// NAME-EDIT: a null clears the name, and the saved document drops the key.
#[test]
fn name_modules_with_null_clears_the_name() {
    let mut session = Session::start(&named());
    session
        .edit(json!([{ "op": "name_modules", "names": { "lpf": null } }]))
        .expect("the edit commits");
    let document = session.document();
    assert!(
        module(&document, "lpf").get("name").is_none(),
        "{document:#}"
    );
    assert_eq!(name_of(&document, "osc").as_deref(), Some("Drone"));
    assert!(session
        .describe("lpf")
        .get("name")
        .map_or(true, Value::is_null));
    session.running.stop();
}

/// NAME-EDIT: one edit names several modules; a name can be replaced.
#[test]
fn name_modules_names_several_modules_and_renames() {
    let mut session = Session::start(&named());
    session
        .edit(json!([{ "op": "name_modules",
                       "names": { "lpf": "Pad filter", "dac": "Out", "osc": null } }]))
        .unwrap();
    let document = session.document();
    assert_eq!(name_of(&document, "lpf").as_deref(), Some("Pad filter"));
    assert_eq!(name_of(&document, "dac").as_deref(), Some("Out"));
    assert_eq!(name_of(&document, "osc"), None);
    session.running.stop();
}

/// NAME-EDIT: "rebuilds no module": the report lists nothing added, removed
/// or rebuilt, no connections change, and every module is untouched.
#[test]
fn a_rename_rebuilds_no_module() {
    let mut session = Session::start(&named());
    let report = session
        .edit(json!([{ "op": "name_modules", "names": { "lpf": "Renamed" } }]))
        .unwrap();
    assert_eq!(report.edit_count, 1);
    assert!(report.added.is_empty(), "{report:?}");
    assert!(report.removed.is_empty(), "{report:?}");
    assert!(report.rebuilt.is_empty(), "{report:?}");
    assert!(report.controls_written.is_empty(), "{report:?}");
    assert_eq!(report.connections_added, 0);
    assert_eq!(report.connections_removed, 0);
    assert_eq!(report.untouched, 3, "all three modules keep their state");
    session.running.stop();
}

/// NAME-EDIT: "audio continues without a glitch". A 220 Hz sine through a
/// filter is smooth, so a rebuild (a phase reset or a filter-state reset) or
/// a dropout shows as a step in the recorded stream. Several renames while
/// playing must leave the stream as smooth as before them.
#[test]
fn renaming_while_playing_leaves_the_audio_continuous() {
    let mut session = Session::start(&named());
    session.render_more(9_600);
    let before = session.recorded().len();
    for (i, name) in ["A", "B", "C", "D"].iter().enumerate() {
        session
            .edit(json!([{ "op": "name_modules",
                           "names": { "lpf": name, "osc": if i % 2 == 0 { Value::Null } else { json!("Drone") } } }]))
            .unwrap();
        session.render_more(2_400);
    }
    session.render_more(4_800);
    let out = session.recorded();
    session.running.stop();

    let max_step = |samples: &[f32]| {
        samples
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    // Skip the first block's start-up transient when judging the baseline.
    let baseline = max_step(&out[256..before]);
    let after = max_step(&out[before..]);
    assert!(
        out[..before].iter().any(|s| s.abs() > 0.05),
        "silent baseline"
    );
    assert!(
        after <= baseline * 1.5 + 0.01,
        "a step of {after} after renaming against {baseline} before: audio glitched"
    );
    assert!(
        out[before..].iter().any(|s| s.abs() > 0.05),
        "audio went silent after renaming"
    );
}

/// NAME-EDIT: "It changes the revision": the host advances its revision when
/// a name_modules batch commits, never when it is refused, and not while it
/// is only queued as a timed edit.
#[test]
fn name_modules_advances_the_revision_like_any_committed_batch() {
    let command: RpcCommand = serde_json::from_value(json!({
        "command": "apply_edits",
        "edits": [{ "op": "name_modules", "names": { "lpf": "Lead filter" } }],
    }))
    .expect("parses as an apply_edits command");
    assert!(matches!(command, RpcCommand::ApplyEdits { .. }));
    assert!(command.advances_revision());
    assert!(command.advances_revision_after(true));
    assert!(
        !command.advances_revision_after(false),
        "a refusal changes nothing"
    );
}

/// NAME-EDIT / NAME-FORMAT: the full request envelope a client sends parses,
/// ticket and all.
#[test]
fn a_name_modules_request_parses_through_the_envelope() {
    let request: RpcRequest = serde_json::from_value(json!({
        "schema_version": 1,
        "kind": "command",
        "command": "apply_edits",
        "mutation": { "id": "editor-1", "issued_at": { "session_id": "s", "revision": 1 } },
        "edits": [
            { "op": "name_modules", "names": { "lpf": "Lead filter", "osc": null } },
        ],
    }))
    .expect("parses");
    assert!(request.check_ticket().is_ok());
}

/// NAME-EDIT: the edit round-trips through its wire form, and a misspelled
/// field is refused like in every other edit.
#[test]
fn name_modules_round_trips_and_refuses_unknown_fields() {
    let wire = json!({ "op": "name_modules", "names": { "lpf": "Lead filter", "osc": null } });
    let edit: StructuralEdit = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&edit).unwrap(), wire);
    assert!(serde_json::from_value::<StructuralEdit>(
        json!({ "op": "name_modules", "name": { "lpf": "x" } })
    )
    .is_err());
}

/// NAME-EDIT: a name for a module that does not exist refuses the whole
/// batch and changes nothing (like every other edit naming a missing module).
#[test]
fn naming_an_unknown_module_is_refused_and_changes_nothing() {
    let mut session = Session::start(&named());
    let before = session.document();
    let refused = session.edit(json!([
        { "op": "name_modules", "names": { "lpf": "Changed", "nope": "Ghost" } },
    ]));
    assert!(refused.is_err(), "expected a refusal, got {refused:?}");
    assert_eq!(session.document(), before);
    session.running.stop();
}

/// NAME-EDIT: add_module takes an optional name.
#[test]
fn add_module_accepts_a_name() {
    let mut session = Session::start(&plain());
    let report = session
        .edit(json!([
            { "op": "add_module", "id": "wobble", "module_type": "lfo",
              "name": "Slow wobble", "config": { "rate": 2.0 } },
            { "op": "add_module", "id": "plainlfo", "module_type": "lfo" },
        ]))
        .expect("commits");
    assert_eq!(report.added, ["wobble", "plainlfo"]);
    let document = session.document();
    assert_eq!(name_of(&document, "wobble").as_deref(), Some("Slow wobble"));
    assert_eq!(name_of(&document, "plainlfo"), None, "no name, no key");
    assert_eq!(session.describe("wobble")["name"], "Slow wobble");
    session.running.stop();
}

/// NAME-EDIT: in one batch, a module added earlier can be named later.
#[test]
fn a_module_added_in_the_batch_can_be_named_in_the_same_batch() {
    let mut session = Session::start(&plain());
    session
        .edit(json!([
            { "op": "add_module", "id": "wobble", "module_type": "lfo" },
            { "op": "name_modules", "names": { "wobble": "Late name" } },
        ]))
        .expect("commits");
    assert_eq!(
        name_of(&session.document(), "wobble").as_deref(),
        Some("Late name")
    );
    session.running.stop();
}

/// NAME-EDIT: names given at load survive an unrelated structural edit.
#[test]
fn names_survive_unrelated_edits() {
    let mut session = Session::start(&named());
    session
        .edit(json!([
            { "op": "add_module", "id": "wobble", "module_type": "lfo" },
            { "op": "set_control", "module_id": "osc", "key": "frequency", "value": 330.0 },
        ]))
        .unwrap();
    let document = session.document();
    assert_eq!(name_of(&document, "osc").as_deref(), Some("Drone"));
    assert_eq!(name_of(&document, "lpf").as_deref(), Some("Lead filter"));
    session.running.stop();
}

/// NAME-EDIT: a renamed document saves with the new name and reloads with it.
#[test]
fn a_renamed_session_saves_and_reloads_its_names() {
    let mut session = Session::start(&plain());
    session
        .edit(json!([{ "op": "name_modules", "names": { "osc": "Drone" } }]))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.json");
    session
        .running
        .document()
        .unwrap()
        .save_to_file(&path)
        .unwrap();
    let reloaded = Invention::from_file(path.to_str().unwrap()).unwrap();
    assert_eq!(name_of(&saved(&reloaded), "osc").as_deref(), Some("Drone"));
    session.running.stop();
}

// =====================================================================================
// Discovery helpers
// =====================================================================================

/// Every registered type's default-config detail, as JSON, via the running
/// registry (the same listing `describe_module_types` full serves).
fn full_listing(session: &Session) -> Vec<Value> {
    let index = session
        .running
        .describe_module_types(&ModuleTypeQuery::default())
        .expect("index");
    let names = index.index.types.clone();
    let mut details = Vec::new();
    for chunk in names.chunks(32) {
        let list = session
            .running
            .describe_module_types(&ModuleTypeQuery {
                types: Some(chunk.to_vec()),
                detail: TypeDetail::Full,
            })
            .expect("full detail");
        let value = serde_json::to_value(list).unwrap();
        for detail in value["details"].as_array().expect("details") {
            if detail["status"] == "available" {
                details.push(detail.clone());
            }
        }
    }
    details
}

fn type_detail(listing: &[Value], name: &str) -> Value {
    listing
        .iter()
        .find(|d| d["type_name"] == name)
        .unwrap_or_else(|| panic!("type {name} has no available full detail"))
        .clone()
}

fn tidy(key: &str) -> String {
    fugue::tidy_key(key).into()
}

/// The (key, label) pairs of a port-label field: a map by port name, or a
/// list aligned with the ports (a missing entry is null or empty).
fn port_labels(detail: &Value, labels: &str, ports: &str) -> Vec<(String, String)> {
    match detail.get(labels) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().expect("a label is text").to_string()))
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let port = detail[ports][i].as_str()?.to_string();
                v.as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| (port, s.to_string()))
            })
            .collect(),
        Some(other) => panic!("unexpected shape for {labels}: {other}"),
    }
}

fn controls(detail: &Value) -> &Vec<Value> {
    detail["controls"].as_array().expect("controls")
}

/// Whether a type declares any label at all (itself, a control, an option or a port).
fn declares_labels(detail: &Value) -> bool {
    detail.get("label").is_some()
        || !port_labels(detail, "input_labels", "inputs").is_empty()
        || !port_labels(detail, "output_labels", "outputs").is_empty()
        || controls(detail).iter().any(|c| {
            c.get("label").is_some()
                || c.get("option_labels")
                    .and_then(Value::as_object)
                    .is_some_and(|m| !m.is_empty())
        })
}

/// A choice control's options. ControlKind is externally tagged on the wire
/// (`{"String": {"options": [...]}}`), so look inside the one variant.
fn options_of(control: &Value) -> Value {
    control["kind"]
        .as_object()
        .and_then(|variants| variants.values().find_map(|v| v.get("options")))
        .cloned()
        .unwrap_or(Value::Null)
}

const WIDGETS: [&str; 5] = ["knob", "slider", "stepper", "segmented", "menu"];

fn listing_session() -> Session {
    Session::start(&plain())
}

// =====================================================================================
// LABELS / WIDGET / TIDY
// =====================================================================================

/// TIDY: no label anywhere in a listing equals what tidying its key gives.
/// Covers the type label, control labels, option labels and port labels of
/// every type the registry lists.
#[test]
fn a_label_is_listed_only_when_it_differs_from_the_tidied_key() {
    let session = listing_session();
    let listing = full_listing(&session);
    assert!(listing.len() > 10, "expected the built-in catalog");
    for detail in &listing {
        let type_name = detail["type_name"].as_str().unwrap();
        if let Some(label) = detail.get("label") {
            let label = label.as_str().expect("a type label is text");
            assert_ne!(
                label,
                tidy(type_name),
                "{type_name}: label equals the tidied name"
            );
        }
        for (field, ports) in [("input_labels", "inputs"), ("output_labels", "outputs")] {
            for (port, label) in port_labels(detail, field, ports) {
                assert_ne!(label, tidy(&port), "{type_name} {field} {port}");
            }
        }
        for control in controls(detail) {
            let key = control["key"].as_str().unwrap();
            if let Some(label) = control.get("label") {
                let label = label.as_str().expect("a control label is text");
                assert_ne!(
                    label,
                    tidy(key),
                    "{type_name}.{key}: label equals the tidied key"
                );
            }
            if let Some(options) = control.get("option_labels") {
                for (option, label) in options.as_object().expect("a map of option to label") {
                    assert_ne!(
                        label.as_str().expect("an option label is text"),
                        tidy(option),
                        "{type_name}.{key} option {option}"
                    );
                }
            }
        }
    }
    session.running.stop();
}

/// WIDGET: only the five named widgets are ever reported, never a null.
#[test]
fn only_the_named_widgets_are_reported() {
    let session = listing_session();
    for detail in full_listing(&session) {
        for control in controls(&detail) {
            if let Some(widget) = control.get("widget") {
                let widget = widget.as_str().unwrap_or_else(|| panic!("widget {widget}"));
                assert!(WIDGETS.contains(&widget), "unknown widget {widget}");
            }
        }
    }
    session.running.stop();
}

/// WIDGET / LABELS: "Nothing is reported when a type declares none": absent
/// values are omitted from the JSON, never null or empty.
#[test]
fn absent_labels_and_widgets_are_omitted_not_null_or_empty() {
    let session = listing_session();
    let listing = full_listing(&session);
    let mut controls_without_widget = 0;
    let mut controls_without_label = 0;
    for detail in &listing {
        for field in ["label", "input_labels", "output_labels"] {
            if let Some(value) = detail.get(field) {
                assert!(!value.is_null(), "{}: {field} is null", detail["type_name"]);
                assert!(value.as_str() != Some(""), "{field} is empty text");
                assert!(
                    value.as_object().map_or(true, |m| !m.is_empty())
                        && value.as_array().map_or(true, |a| !a.is_empty()),
                    "{}: {field} is an empty collection",
                    detail["type_name"]
                );
            }
        }
        for control in controls(detail) {
            for field in ["label", "widget", "option_labels"] {
                if let Some(value) = control.get(field) {
                    assert!(!value.is_null(), "{field} is null on {control}");
                    assert!(
                        value.as_object().map_or(true, |m| !m.is_empty()),
                        "{field} is an empty map on {control}"
                    );
                }
            }
            controls_without_widget += control.get("widget").is_none() as usize;
            controls_without_label += control.get("label").is_none() as usize;
        }
    }
    assert!(
        controls_without_widget > 0,
        "every control declares a widget?"
    );
    assert!(
        controls_without_label > 0,
        "every control declares a label?"
    );
    session.running.stop();
}

/// WIDGET / LABELS: ControlMeta's wire form omits the new fields when absent,
/// and a ControlMeta written before them still reads.
#[test]
fn control_meta_omits_absent_label_widget_and_option_labels() {
    let session = listing_session();
    let listing = full_listing(&session);
    // A control as a type that declares nothing would list it: any listed
    // control with the declared fields stripped.
    let mut plain_control = listing
        .iter()
        .flat_map(|detail| controls(detail).iter())
        .find(|c| c.get("label").is_none() && c.get("widget").is_none())
        .or_else(|| listing.iter().flat_map(|d| controls(d).iter()).next())
        .expect("some control is listed")
        .clone();
    for field in ["label", "widget", "option_labels"] {
        plain_control.as_object_mut().unwrap().remove(field);
    }
    let meta: ControlMeta = serde_json::from_value(plain_control.clone()).expect("reads");
    let back = serde_json::to_value(&meta).unwrap();
    for field in ["label", "widget", "option_labels"] {
        assert!(
            back.get(field).is_none(),
            "{field} serialized when absent: {back}"
        );
    }
    assert_eq!(back, plain_control, "round trip");

    // A control that declares them keeps them through ControlMeta.
    let mut declared = plain_control;
    declared["label"] = json!("A Label");
    declared["widget"] = json!("slider");
    let meta: ControlMeta = serde_json::from_value(declared.clone()).expect("reads");
    assert_eq!(serde_json::to_value(&meta).unwrap(), declared);
    for widget in WIDGETS {
        declared["widget"] = json!(widget);
        assert!(
            serde_json::from_value::<ControlMeta>(declared.clone()).is_ok(),
            "widget {widget}"
        );
    }
    declared["widget"] = json!("dial-of-doom");
    assert!(serde_json::from_value::<ControlMeta>(declared).is_err());
    session.running.stop();
}

/// LABELS: option labels belong to choice controls and name only their
/// options.
#[test]
fn option_labels_name_only_the_options_of_a_choice_control() {
    let session = listing_session();
    let mut seen = 0;
    for detail in full_listing(&session) {
        for control in controls(&detail) {
            let Some(labels) = control.get("option_labels") else {
                continue;
            };
            seen += 1;
            let listed = options_of(control);
            let options: BTreeSet<&str> = listed
                .as_array()
                .unwrap_or_else(|| panic!("option_labels on a control without options: {control}"))
                .iter()
                .map(|o| o.as_str().unwrap())
                .collect();
            for option in labels.as_object().unwrap().keys() {
                assert!(
                    options.contains(option.as_str()),
                    "{control}: stray option {option}"
                );
            }
        }
    }
    assert!(seen > 0, "no type labels the values of a choice control");
    session.running.stop();
}

/// FIRST-PARTY / LABELS: the clock, the filter and the oscillator each
/// declare labels (for the type, a control, a choice value or a port).
#[test]
fn the_clock_the_filter_and_the_oscillator_declare_labels() {
    let session = listing_session();
    let listing = full_listing(&session);
    for name in ["clock", "filter", "oscillator"] {
        let detail = type_detail(&listing, name);
        assert!(
            declares_labels(&detail),
            "{name} declares no labels: {detail:#}"
        );
    }
    session.running.stop();
}

/// FIRST-PARTY / WIDGET: the first-party pattern includes a preferred
/// widget: some control of the three declares one.
#[test]
fn the_first_party_pattern_declares_a_widget() {
    let session = listing_session();
    let listing = full_listing(&session);
    let widgets: usize = ["clock", "filter", "oscillator"]
        .iter()
        .map(|name| {
            controls(&type_detail(&listing, name))
                .iter()
                .filter(|c| c.get("widget").is_some())
                .count()
        })
        .sum();
    assert!(
        widgets > 0,
        "none of clock, filter, oscillator declares a widget"
    );
    session.running.stop();
}

/// FIRST-PARTY / LABELS: the choice controls of the filter and oscillator
/// (their waveform and filter type) label their values, and the labels are
/// real text.
#[test]
fn the_oscillator_and_filter_label_their_choices_or_controls() {
    let session = listing_session();
    let listing = full_listing(&session);
    for name in ["filter", "oscillator"] {
        let detail = type_detail(&listing, name);
        let labelled_controls = controls(&detail)
            .iter()
            .filter(|c| c.get("label").is_some() || c.get("option_labels").is_some())
            .count();
        assert!(labelled_controls > 0, "{name}: no control or choice label");
    }
    session.running.stop();
}

/// LABELS: describe_module for a type (no running instance) reports the same
/// labels and widgets the listing does.
#[test]
fn describe_module_of_a_type_reports_what_the_listing_reports() {
    let session = listing_session();
    let listing = full_listing(&session);
    for name in ["clock", "filter", "oscillator"] {
        let listed = type_detail(&listing, name);
        let query = DescribeModuleQuery {
            module_type: Some(name.to_string()),
            ..Default::default()
        };
        let described =
            serde_json::to_value(session.running.describe_module(&query).unwrap()).unwrap();
        for field in ["label", "input_labels", "output_labels"] {
            assert_eq!(described.get(field), listed.get(field), "{name} {field}");
        }
        for (a, b) in controls(&described).iter().zip(controls(&listed)) {
            for field in ["label", "option_labels", "widget"] {
                assert_eq!(a.get(field), b.get(field), "{name}.{} {field}", a["key"]);
            }
        }
    }
    session.running.stop();
}

/// DESCRIBE: describe_module of a running module reports its name, its
/// labels, and its id, so an agent sees the human name and still addresses
/// the module by id.
#[test]
fn describe_module_of_a_running_module_shows_name_labels_and_id() {
    let session = Session::start(&named());
    let listing = full_listing(&session);
    let description = session.describe("lpf");
    assert_eq!(description["module_id"], "lpf");
    assert_eq!(description["name"], "Lead filter");
    assert_eq!(description["type_name"], "filter");
    let listed = type_detail(&listing, "filter");
    for field in ["label", "input_labels", "output_labels"] {
        assert_eq!(description.get(field), listed.get(field), "{field}");
    }
    assert!(
        controls(&description)
            .iter()
            .any(|c| c.get("label").is_some()
                || c.get("widget").is_some()
                || c.get("option_labels").is_some()),
        "a running filter reports no control label or widget"
    );
    // Addressing by id still works for everything else: a control read.
    assert!(session.running.get_control("lpf", "cutoff").is_ok());
    // An unnamed module reports no name.
    let unnamed = session.describe("dac");
    assert!(unnamed.get("name").map_or(true, Value::is_null));
    assert_eq!(unnamed["module_id"], "dac");
    session.running.stop();
}

/// LABELS: the name of a module is not a label of its type: renaming never
/// changes what the type reports.
#[test]
fn a_module_name_does_not_change_its_type_labels() {
    let named_session = Session::start(&named());
    let plain_session = Session::start(&plain());
    let (a, b) = (named_session.describe("lpf"), plain_session.describe("lpf"));
    for field in ["label", "input_labels", "output_labels", "controls"] {
        assert_eq!(a.get(field), b.get(field), "{field}");
    }
    named_session.running.stop();
    plain_session.running.stop();
}
