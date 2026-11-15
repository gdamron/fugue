//! Arranging a development's inside as the piece goes on: refused while a
//! timed edit waits, and one file reached by two spellings or a link.

use serde_json::{json, Value};

use super::*;
use crate::invention::layout::Position;
use crate::rpc::{RpcErrorCode, RuntimeRevision, StructuralEdit};

fn names(path: &[&str]) -> Vec<String> {
    path.iter().map(|name| name.to_string()).collect()
}

fn at(x: f64) -> Position {
    Position { x, y: 1.0 }
}

fn move_inside(path: &[&str], id: &str, x: f64) -> StructuralEdit {
    StructuralEdit::MoveModules {
        development: Some(names(path)),
        positions: [(id.to_string(), Some(at(x)))].into_iter().collect(),
    }
}

fn layout_of(running: &Settled, path: &[&str]) -> std::collections::BTreeMap<String, Position> {
    let revision = RuntimeRevision {
        session_id: "session".into(),
        revision: 1,
    };
    let read = running.development(&names(path), revision).unwrap();
    read.definition.layout.modules
}

fn voice() -> Value {
    json!({
        "version": "1.0.0",
        "modules": [{ "id": "osc", "type": "oscillator" }, { "id": "amp", "type": "vca" }],
        "connections": [{ "from": "osc", "from_port": "audio", "to": "amp", "to_port": "audio" }],
        "outputs": [{ "name": "audio", "from": "amp", "from_port": "audio" }]
    })
}

fn piece(developments: Value) -> Value {
    json!({
        "version": "1.0.0",
        "developments": developments,
        "modules": [{ "id": "v", "type": "voice" }, { "id": "dac", "type": "dac" }],
        "connections": [{ "from": "v", "from_port": "audio", "to": "dac", "to_port": "audio" }]
    })
}

fn inline_layout(document: &Invention) -> std::collections::BTreeMap<String, Position> {
    let definition = document.developments[0].definition.as_deref().unwrap();
    definition.layout.modules.clone()
}

#[test]
fn a_move_inside_waits_for_no_timed_reload_it_is_refused() {
    let declared = json!([{ "name": "voice", "definition": voice() }]);
    let mut running = start(&piece(declared.clone()).to_string());
    let mut next = piece(declared);
    next["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "lfo", "type": "lfo" }));
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let next = Invention::from_json(&next.to_string()).unwrap();
    running.reload_at(next, &when, None).expect("queued");

    let refused = running.apply_edits(&[move_inside(&["voice"], "osc", 3.0)]);
    assert_eq!(refused.unwrap_err().code, RpcErrorCode::InvalidRequest);
    running.pump.render(64);
    running.live.adopt_landed();
    running
        .apply_edits(&[move_inside(&["voice"], "osc", 3.0)])
        .unwrap();
    assert_eq!(inline_layout(&running.document().unwrap())["osc"], at(3.0));
    assert_eq!(layout_of(&running, &["voice"])["osc"], at(3.0));
}

#[cfg(unix)]
#[test]
fn one_file_under_two_spellings_and_a_link_is_one_development() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("dev").join("voice.json");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, serde_json::to_vec_pretty(&voice()).unwrap()).unwrap();
    let link = dir.path().join("linked.json");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let declared = json!([
        { "name": "voice", "path": "dev/voice.json" },
        { "name": "again", "path": "dev/../dev/voice.json" },
        { "name": "linked", "path": "linked.json" }
    ]);
    let path = dir.path().join("piece.json");
    std::fs::write(&path, piece(declared).to_string()).unwrap();
    let document = Invention::from_file(&path.to_string_lossy()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(document).unwrap();
    let mut running = Settled::start(runtime);

    running
        .apply_edits(&[move_inside(&["linked"], "osc", 4.0)])
        .unwrap();

    for name in ["voice", "again", "linked"] {
        assert_eq!(layout_of(&running, &[name])["osc"], at(4.0), "{name}");
    }
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    let written = Invention::from_file(&real.to_string_lossy()).unwrap();
    assert_eq!(written.layout.modules["osc"], at(4.0));
    let revision = RuntimeRevision {
        session_id: "session".into(),
        revision: 1,
    };
    let read = running.development(&names(&["again"]), revision).unwrap();
    let canonical = std::fs::canonicalize(&real).unwrap();
    let file = crate::rpc::DevelopmentSavedIn::File {
        path: canonical.display().to_string(),
    };
    assert_eq!(read.saved_in, file);
    // A second move through another spelling finds the file as it left it.
    running
        .apply_edits(&[move_inside(&["again"], "amp", 5.0)])
        .unwrap();
    assert_eq!(layout_of(&running, &["voice"])["amp"], at(5.0));
}
