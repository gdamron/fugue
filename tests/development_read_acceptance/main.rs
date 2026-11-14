//! Acceptance tests for reading a loaded development's inside with
//! `RpcCommand::GetDevelopment`, written from the documented contract alone.
//!
//! They serve the criterion "Developments used inline, by file path and from
//! an installed package all open", and "A development inside a development
//! opens the same way".
//!
//! Run with `cargo test --test development_read_acceptance`.

use std::path::Path;

use fugue::rpc::{
    DevelopmentDeclaration, DevelopmentSnapshot, RpcError, RpcErrorCode, RuntimeRevision,
    MAX_DEVELOPMENT_BYTES,
};
use fugue::{AudioBackend, Invention, InventionBuilder, RpcCommand, RunningInvention};
use serde_json::{json, Value};

const RATE: u32 = 48_000;

struct Silent;

impl AudioBackend for Silent {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn start(
        &mut self,
        _render: Box<dyn FnMut(&mut [f32], &mut [f32]) + Send>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
    fn stop(&mut self) {}
}

fn revision() -> RuntimeRevision {
    RuntimeRevision {
        session_id: "acceptance".into(),
        revision: 7,
    }
}

/// A development with one oscillator, an output, a control and a layout.
fn voice(module: &str, x: f64) -> Value {
    json!({
        "version": "1.0.0",
        "modules": [{ "id": module, "type": "oscillator", "config": { "frequency": 220.0 } }],
        "connections": [],
        "outputs": [{ "name": "audio", "from": module, "from_port": "audio" }],
        "controls": [{ "name": "pitch", "module": module, "control": "frequency" }],
        "layout": { "modules": { module: { "x": x, "y": 80.0 } } }
    })
}

/// A piece that declares `developments` and plays one instance of each name
/// in `play` into the dac.
fn piece(developments: Value, play: &[&str]) -> Value {
    let mut modules = vec![json!({ "id": "dac", "type": "dac" })];
    let mut connections = vec![];
    for name in play {
        modules.push(json!({ "id": format!("{name}_1"), "type": name }));
        connections.push(json!({
            "from": format!("{name}_1"), "from_port": "audio", "to": "dac", "to_port": "audio"
        }));
    }
    json!({
        "version": "1.0.0",
        "developments": developments,
        "modules": modules,
        "connections": connections
    })
}

fn start(invention: Invention) -> RunningInvention {
    let (runtime, _) = InventionBuilder::new(RATE)
        .build(invention)
        .expect("the piece builds");
    runtime
        .start_with_backend(Silent)
        .expect("the piece starts")
}

fn start_json(document: Value) -> RunningInvention {
    start(serde_json::from_value(document).expect("a valid document"))
}

fn read(running: &RunningInvention, path: &[&str]) -> Result<DevelopmentSnapshot, RpcError> {
    let path: Vec<String> = path.iter().map(|s| s.to_string()).collect();
    running.development(&path, revision())
}

fn expect_invalid(running: &RunningInvention, path: &[&str]) {
    let error = read(running, path).expect_err(&format!("{path:?} resolves to nothing"));
    assert_eq!(
        error.code,
        RpcErrorCode::InvalidRequest,
        "{path:?}: {error:?}"
    );
}

fn write(dir: &Path, name: &str, document: &Value) {
    std::fs::write(
        dir.join(name),
        serde_json::to_string_pretty(document).unwrap(),
    )
    .unwrap();
}

fn field(snapshot: &DevelopmentSnapshot, key: &str) -> Value {
    serde_json::to_value(&snapshot.definition).unwrap()[key].clone()
}

/// Criterion: developments used inline open, keeping their own layout and
/// exposed outputs and controls, and say they were declared inline.
#[test]
fn an_inline_development_opens_with_its_own_layout_and_exposed_surface() {
    let running = start_json(piece(
        json!([{ "name": "voice", "definition": voice("osc", 40.0) }]),
        &["voice"],
    ));
    let snapshot = read(&running, &["voice"]).expect("an inline development reads");
    assert_eq!(snapshot.declared, DevelopmentDeclaration::Inline);
    assert_eq!(snapshot.path, ["voice"]);
    assert_eq!(snapshot.revision, revision());
    let expected = voice("osc", 40.0);
    for key in ["modules", "outputs", "controls", "layout"] {
        assert_eq!(field(&snapshot, key), expected[key], "{key}");
    }
    running.stop();
}

/// Criterion: developments used by file path open, and say `path`.
#[test]
fn a_development_declared_by_file_path_opens() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "voice.json", &voice("file_osc", 12.0));
    write(
        dir.path(),
        "piece.json",
        &piece(
            json!([{ "name": "voice", "path": "voice.json" }]),
            &["voice"],
        ),
    );
    let piece_path = dir.path().join("piece.json");
    let running = start(Invention::from_file(piece_path.to_str().unwrap()).unwrap());
    let snapshot = read(&running, &["voice"]).expect("a path-declared development reads");
    assert_eq!(snapshot.declared, DevelopmentDeclaration::Path);
    let expected = voice("file_osc", 12.0);
    for key in ["modules", "outputs", "controls", "layout"] {
        assert_eq!(field(&snapshot, key), expected[key], "{key}");
    }
    running.stop();
}

/// Criterion: developments from an installed package open, and say `ref`.
/// The only test that sets `FUGUE_PACKS_DIR`.
#[test]
fn a_development_from_an_installed_package_opens() {
    let packs = tempfile::tempdir().unwrap();
    let home = packs.path().join("fugue.test.voice").join("1.0.0");
    std::fs::create_dir_all(&home).unwrap();
    write(
        &home,
        "fugue.pkg.json",
        &json!({
            "id": "fugue.test.voice", "version": "1.0.0", "kind": "development",
            "license": "MIT", "authors": [{ "name": "Test" }],
            "targets": ["external-agent"], "entry": { "development": "voice.json" }
        }),
    );
    write(&home, "voice.json", &voice("pkg_osc", 5.0));
    std::env::set_var("FUGUE_PACKS_DIR", packs.path());

    let running = start_json(piece(
        json!([{ "name": "chime", "ref": { "package": "fugue.test.voice", "version": "1.0.0" } }]),
        &["chime"],
    ));
    let snapshot = read(&running, &["chime"]).expect("a package development reads");
    assert_eq!(snapshot.declared, DevelopmentDeclaration::Ref);
    let expected = voice("pkg_osc", 5.0);
    for key in ["modules", "outputs", "controls", "layout"] {
        assert_eq!(field(&snapshot, key), expected[key], "{key}");
    }
    running.stop();
}

/// Criterion: a development inside a development opens the same way, whether
/// declared in the outer definition or inherited from the piece.
#[test]
fn a_nested_development_opens_declared_inside_or_inherited() {
    let mut outer = voice("osc", 1.0);
    outer["developments"] = json!([{ "name": "env", "definition": voice("env_osc", 9.0) }]);
    outer["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "env_1", "type": "env" }));
    // `shared` is declared by the piece before `voice`, so `voice` inherits it.
    let mut inner = voice("osc", 2.0);
    inner["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "shared_1", "type": "shared" }));
    let running = start_json(piece(
        json!([
            { "name": "shared", "definition": voice("shared_osc", 3.0) },
            { "name": "voice", "definition": inner },
            { "name": "deep", "definition": outer }
        ]),
        &["voice", "deep"],
    ));

    let declared_inside = read(&running, &["deep", "env"]).expect("a nested development reads");
    assert_eq!(declared_inside.declared, DevelopmentDeclaration::Inline);
    assert_eq!(declared_inside.path, ["deep", "env"]);
    assert_eq!(
        field(&declared_inside, "layout"),
        voice("env_osc", 9.0)["layout"]
    );

    let inherited = read(&running, &["voice", "shared"]).expect("an inherited development reads");
    assert_eq!(inherited.path, ["voice", "shared"]);
    assert_eq!(
        field(&inherited, "layout"),
        voice("shared_osc", 3.0)["layout"]
    );
    assert_eq!(
        field(&inherited, "controls"),
        voice("shared_osc", 3.0)["controls"]
    );

    // The outer definition keeps naming the development it declares.
    let deep = read(&running, &["deep"]).unwrap();
    assert_eq!(field(&deep, "developments")[0]["name"], "env");
    running.stop();
}

/// Criterion (opening what exists, refusing what does not): an empty path or a
/// name that resolves to nothing, at any depth, is `InvalidRequest`.
#[test]
fn an_empty_path_or_unknown_name_is_an_invalid_request() {
    let mut outer = voice("osc", 1.0);
    outer["developments"] = json!([{ "name": "env", "definition": voice("env_osc", 9.0) }]);
    let running = start_json(piece(
        json!([{ "name": "deep", "definition": outer }]),
        &["deep"],
    ));
    expect_invalid(&running, &[]);
    expect_invalid(&running, &["ghost"]);
    expect_invalid(&running, &["deep", "ghost"]);
    expect_invalid(&running, &["env"]);
    // A module's id is not a development's name.
    expect_invalid(&running, &["deep_1"]);
    running.stop();
}

/// Criterion (nested ones resolve as the builder does): a name inherited from
/// the piece resolves only when the piece declares it before the development.
#[test]
fn a_nested_name_resolves_only_from_an_earlier_declaration() {
    let running = start_json(piece(
        json!([
            { "name": "early", "definition": voice("early_osc", 1.0) },
            { "name": "deep", "definition": voice("osc", 2.0) },
            { "name": "late", "definition": voice("late_osc", 3.0) }
        ]),
        &[],
    ));
    assert!(read(&running, &["deep", "early"]).is_ok());
    expect_invalid(&running, &["deep", "late"]);
    // Both are still readable as the piece's own developments.
    assert!(read(&running, &["late"]).is_ok());
    running.stop();
}

/// Criterion: the result reflects what the piece loaded; a file edited on disk
/// afterward shows only after a reload.
#[test]
fn an_edit_to_a_development_file_after_load_does_not_show() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "voice.json", &voice("before", 10.0));
    write(
        dir.path(),
        "piece.json",
        &piece(
            json!([{ "name": "voice", "path": "voice.json" }]),
            &["voice"],
        ),
    );
    let piece_path = dir.path().join("piece.json");
    let running = start(Invention::from_file(piece_path.to_str().unwrap()).unwrap());

    write(dir.path(), "voice.json", &voice("after", 99.0));
    let snapshot = read(&running, &["voice"]).unwrap();
    assert_eq!(field(&snapshot, "modules")[0]["id"], "before");
    assert_eq!(field(&snapshot, "layout")["modules"]["before"]["x"], 10.0);
    running.stop();
}

/// Criterion: an answer over the bound is refused as `ResponseTooLarge`, not
/// cut; one under it is answered whole.
#[test]
fn a_definition_over_the_bound_is_refused_not_truncated() {
    let big = |bytes: usize| {
        let mut definition = voice("osc", 1.0);
        definition["description"] = json!("x".repeat(bytes));
        start_json(piece(
            json!([{ "name": "big", "definition": definition }]),
            &["big"],
        ))
    };
    let large = big(MAX_DEVELOPMENT_BYTES + 1_000);
    let error = read(&large, &["big"]).expect_err("over the bound");
    assert_eq!(error.code, RpcErrorCode::ResponseTooLarge, "{error:?}");
    large.stop();

    let roomy = big(MAX_DEVELOPMENT_BYTES / 2);
    let snapshot = read(&roomy, &["big"]).expect("under the bound is answered");
    let whole = serde_json::to_vec(&snapshot).unwrap();
    assert!(whole.len() <= MAX_DEVELOPMENT_BYTES);
    assert_eq!(
        field(&snapshot, "description").as_str().unwrap().len(),
        MAX_DEVELOPMENT_BYTES / 2
    );
    roomy.stop();
}

/// Contract: the command moves no revision and has the documented wire shape.
#[test]
fn get_development_reads_without_advancing_the_revision_and_round_trips() {
    let command = RpcCommand::GetDevelopment {
        path: vec!["voice".into(), "env".into()],
    };
    assert!(!command.advances_revision());
    let wire = json!({ "command": "get_development", "path": ["voice", "env"] });
    assert_eq!(serde_json::to_value(&command).unwrap(), wire);
    let parsed: RpcCommand = serde_json::from_value(wire).unwrap();
    assert_eq!(parsed, command);
}
