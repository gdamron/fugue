//! Reading a development's inside from the running piece: inline, by path,
//! from a package and nested, each as loaded, and the refusals.

use serde_json::{json, Value};

use super::*;
use crate::rpc::{DevelopmentDeclaration, DevelopmentSnapshot, RpcErrorCode, RuntimeRevision};

fn revision() -> RuntimeRevision {
    RuntimeRevision {
        session_id: "session".into(),
        revision: 7,
    }
}

fn names(path: &[&str]) -> Vec<String> {
    path.iter().map(|name| name.to_string()).collect()
}

fn read(running: &Settled, path: &[&str]) -> Result<DevelopmentSnapshot, crate::RpcError> {
    running.development(&names(path), revision())
}

fn module_ids(snapshot: &DevelopmentSnapshot) -> Vec<&str> {
    let modules = snapshot.definition.modules.iter();
    modules.map(|module| module.id.as_str()).collect()
}

/// A development of one oscillator `id` out through `audio`.
fn tone(id: &str) -> Value {
    json!({
        "modules": [{ "id": id, "type": "oscillator" }],
        "connections": [],
        "outputs": [{ "name": "audio", "from": id, "from_port": "audio" }]
    })
}

/// A piece declaring `developments`, playing one instance of `played`.
fn piece(developments: Value, played: &str) -> Value {
    json!({
        "developments": developments,
        "modules": [{ "id": "v", "type": played }, { "id": "dac", "type": "dac" }],
        "connections": [{ "from": "v", "from_port": "audio", "to": "dac", "to_port": "audio" }]
    })
}

fn write(path: &std::path::Path, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn start_file(path: &std::path::Path) -> Settled {
    let document = Invention::from_file(&path.to_string_lossy()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(document).unwrap();
    Settled::start(runtime)
}

#[test]
fn an_inline_development_reads_with_its_layout_and_interface() {
    let mut voice = tone("osc");
    voice["layout"] = json!({ "modules": { "osc": { "x": 40.0, "y": 20.0 } } });
    voice["controls"] = json!([{ "name": "pitch", "module": "osc", "control": "frequency" }]);
    let running =
        start(&piece(json!([{ "name": "voice", "definition": voice }]), "voice").to_string());

    let snapshot = read(&running, &["voice"]).unwrap();
    assert_eq!(snapshot.declared, DevelopmentDeclaration::Inline);
    assert_eq!(snapshot.path, names(&["voice"]));
    assert_eq!(snapshot.revision, revision());
    assert_eq!(module_ids(&snapshot), ["osc"]);
    assert_eq!(snapshot.definition.outputs[0].name, "audio");
    assert_eq!(snapshot.definition.controls[0].name, "pitch");
    let placed = &snapshot.definition.layout.modules["osc"];
    assert_eq!((placed.x, placed.y), (40.0, 20.0));
}

#[test]
fn path_developments_read_from_what_was_loaded_nested_ones_too() {
    let dir = tempfile::tempdir().unwrap();
    let mut pad = tone("inner_voice");
    pad["modules"][0]["type"] = json!("grain");
    pad["developments"] = json!([{ "name": "grain", "path": "parts/grain.json" }]);
    write(&dir.path().join("pad.json"), &pad);
    write(&dir.path().join("parts/grain.json"), &tone("grain_osc"));
    let document = piece(json!([{ "name": "pad", "path": "pad.json" }]), "pad");
    write(&dir.path().join("piece.json"), &document);
    let running = start_file(&dir.path().join("piece.json"));
    // What plays is what was loaded: a later edit on disk is not read.
    write(&dir.path().join("parts/grain.json"), &tone("edited"));

    let outer = read(&running, &["pad"]).unwrap();
    assert_eq!(outer.declared, DevelopmentDeclaration::Path);
    assert_eq!(module_ids(&outer), ["inner_voice"]);
    let inner = read(&running, &["pad", "grain"]).unwrap();
    assert_eq!(inner.declared, DevelopmentDeclaration::Path);
    assert_eq!(module_ids(&inner), ["grain_osc"]);
}

#[test]
fn a_package_development_reads_by_its_reference() {
    let dir = tempfile::tempdir().unwrap();
    let packs = dir.path().join("packs");
    let root = packs.join("test.dev.voice").join("1.0.0");
    write(
        &root.join("fugue.pkg.json"),
        &json!({"id":"test.dev.voice","version":"1.0.0","kind":"development","license":"MIT",
                "authors":[{"name":"Test"}],"targets":["external-agent"],
                "entry":{"development":"voice.json"}}),
    );
    let mut voice = tone("pkg_osc");
    voice["title"] = json!("Package voice");
    write(&root.join("voice.json"), &voice);
    crate::pkg::audio_asset::with_packs_dir(&packs, || {
        let reference = json!({ "package": "test.dev.voice", "version": "1.0.0" });
        let developments = json!([{ "name": "shared", "ref": reference }]);
        let running = start(&piece(developments, "shared").to_string());

        let snapshot = read(&running, &["shared"]).unwrap();
        assert_eq!(snapshot.declared, DevelopmentDeclaration::Ref);
        assert_eq!(module_ids(&snapshot), ["pkg_osc"]);
        assert_eq!(snapshot.definition.title.as_deref(), Some("Package voice"));
    });
}

#[test]
fn a_nested_name_resolves_as_its_builder_sees_it() {
    let mut voice = tone("unused");
    voice["modules"] = json!([{ "id": "t", "type": "tone" }, { "id": "e", "type": "echo" }]);
    voice["outputs"] = json!([{ "name": "audio", "from": "t", "from_port": "audio" }]);
    voice["developments"] = json!([{ "name": "echo", "definition": tone("echo_osc") }]);
    let developments = json!([
        { "name": "tone", "definition": tone("tone_osc") },
        { "name": "voice", "definition": voice },
        { "name": "late", "definition": tone("late_osc") },
    ]);
    let running = start(&piece(developments, "voice").to_string());

    // Declared inside the development.
    assert_eq!(
        module_ids(&read(&running, &["voice", "echo"]).unwrap()),
        ["echo_osc"]
    );
    // Inherited from the piece, which declares it before the development.
    assert_eq!(
        module_ids(&read(&running, &["voice", "tone"]).unwrap()),
        ["tone_osc"]
    );
    // Declared after it, so not visible inside it.
    let late = read(&running, &["voice", "late"]).unwrap_err();
    assert_eq!(late.code, RpcErrorCode::InvalidRequest);
    assert!(late.message.contains("inside 'voice'"), "{}", late.message);
}

#[test]
fn unknown_and_empty_paths_are_refused() {
    let developments = json!([{ "name": "voice", "definition": tone("osc") }]);
    let running = start(&piece(developments, "voice").to_string());

    for path in [
        &[][..],
        &["missing"][..],
        &["voice", "missing"][..],
        &["dac"][..],
    ] {
        let error = read(&running, path).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::InvalidRequest, "{path:?}");
    }
}

#[test]
fn a_development_past_the_bound_is_too_large_not_cut() {
    let mut voice = tone("osc");
    voice["description"] = json!("a".repeat(crate::rpc::MAX_DEVELOPMENT_BYTES));
    let developments = json!([{ "name": "voice", "definition": voice }]);
    let running = start(&piece(developments, "voice").to_string());

    let error = read(&running, &["voice"]).unwrap_err();
    assert_eq!(error.code, RpcErrorCode::ResponseTooLarge);
    assert!(!crate::RpcCommand::GetDevelopment {
        path: names(&["voice"])
    }
    .advances_revision());
}

#[test]
fn a_reload_reads_the_definitions_it_loaded() {
    let _ungoverned = crate::pkg::lock::ungoverned_here();
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("voice.json"), &tone("before"));
    let document = piece(json!([{ "name": "voice", "path": "voice.json" }]), "voice");
    write(&dir.path().join("piece.json"), &document);
    let mut running = start_file(&dir.path().join("piece.json"));
    write(&dir.path().join("voice.json"), &tone("after"));
    assert_eq!(module_ids(&read(&running, &["voice"]).unwrap()), ["before"]);

    let reloaded = Invention::from_file(&dir.path().join("piece.json").to_string_lossy()).unwrap();
    running.reload(reloaded).expect("reload applies");
    assert_eq!(module_ids(&read(&running, &["voice"]).unwrap()), ["after"]);
}
