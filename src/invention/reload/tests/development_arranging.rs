//! Arranging a development's inside as the piece goes on: refused while a
//! timed edit waits, and in a development file, not arranged yet.

use serde_json::{json, Value};

use super::*;
use crate::invention::layout::Position;
use crate::rpc::{
    DevelopmentSavedIn, EditFailureReason, RpcErrorCode, RuntimeRevision, StructuralEdit,
};

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

#[test]
fn a_development_file_is_not_arranged_yet() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("voice.json");
    let written = serde_json::to_vec_pretty(&voice()).unwrap();
    std::fs::write(&file, &written).unwrap();
    let path = dir.path().join("piece.json");
    let declared = json!([{ "name": "voice", "path": "voice.json" }]);
    std::fs::write(&path, piece(declared).to_string()).unwrap();
    let document = Invention::from_file(&path.to_string_lossy()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(document).unwrap();
    let mut running = Settled::start(runtime);
    let revision = RuntimeRevision {
        session_id: "session".into(),
        revision: 1,
    };
    let read = running.development(&names(&["voice"]), revision).unwrap();
    assert!(matches!(read.saved_in, DevelopmentSavedIn::ReadOnly { .. }));

    let refused = running.apply_edits(&[move_inside(&["voice"], "osc", 4.0)]);
    let failure = refused.unwrap_err().edit.expect("an edit refusal");
    assert_eq!(failure.reason, EditFailureReason::ReadOnlyDevelopment);
    assert_eq!(std::fs::read(&file).unwrap(), written);
    assert!(layout_of(&running, &["voice"]).is_empty());
}
