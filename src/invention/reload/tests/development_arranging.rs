//! Arranging a development's inside as the piece goes on: a timed reload
//! landing after the move, and one file reached by two spellings or a link.

use serde_json::{json, Value};

use super::*;
use crate::invention::layout::Position;
use crate::rpc::{RuntimeRevision, StructuralEdit};

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
fn a_timed_reload_landing_keeps_an_inline_move_made_while_it_waits() {
    let declared = json!([{ "name": "voice", "definition": voice() }]);
    let mut running = start(&piece(declared.clone()).to_string());
    // The reload adds a module and leaves the inside alone.
    let mut next = piece(declared);
    next["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "lfo", "type": "lfo" }));
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let next = Invention::from_json(&next.to_string()).unwrap();
    running.reload_at(next, &when, None).expect("queued");

    running
        .apply_edits(&[move_inside(&["voice"], "osc", 3.0)])
        .unwrap();
    running.pump.render(64);
    running.live.adopt_landed();

    let document = running.document().unwrap();
    assert!(
        document.modules.iter().any(|spec| spec.id == "lfo"),
        "landed"
    );
    assert_eq!(inline_layout(&document)["osc"], at(3.0));
    assert_eq!(layout_of(&running, &["voice"])["osc"], at(3.0));
}

#[test]
fn a_timed_reload_that_moves_inside_itself_keeps_its_own_arrangement() {
    let declared = json!([{ "name": "voice", "definition": voice() }]);
    let mut running = start(&piece(declared).to_string());
    let mut moved = voice();
    moved["layout"] = json!({ "modules": { "amp": { "x": 9.0, "y": 1.0 } } });
    let mut next = piece(json!([{ "name": "voice", "definition": moved }]));
    next["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "lfo", "type": "lfo" }));
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let next = Invention::from_json(&next.to_string()).unwrap();
    running.reload_at(next, &when, None).expect("queued");

    running
        .apply_edits(&[move_inside(&["voice"], "osc", 3.0)])
        .unwrap();
    running.pump.render(64);
    running.live.adopt_landed();

    let layout = inline_layout(&running.document().unwrap());
    assert_eq!(layout.get("amp"), Some(&at(9.0)), "the reload's own move");
    assert_eq!(
        layout.get("osc"),
        None,
        "the reload replaced the inside's layout"
    );
    let read = layout_of(&running, &["voice"]);
    assert_eq!((read.get("amp"), read.get("osc")), (Some(&at(9.0)), None));
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

/// Lands the timed reload `next` (`piece` with an lfo added) on `running`.
fn land(running: &mut Settled, next: Value, between: impl FnOnce(&mut Settled)) {
    let mut next = next;
    next["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "lfo", "type": "lfo" }));
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let next = Invention::from_json(&next.to_string()).unwrap();
    running.reload_at(next, &when, None).expect("queued");
    between(running);
    running.pump.render(64);
    running.live.adopt_landed();
    assert!(running
        .document()
        .unwrap()
        .modules
        .iter()
        .any(|m| m.id == "lfo"));
}

#[test]
fn a_timed_reload_that_lands_is_what_get_development_reads() {
    let declared = json!([{ "name": "voice", "definition": voice() }]);
    let mut running = start(&piece(declared).to_string());
    let mut renamed = voice();
    renamed["modules"][0]["name"] = json!("Body");
    renamed["layout"] = json!({ "modules": { "osc": { "x": 2.0, "y": 1.0 } } });
    land(
        &mut running,
        piece(json!([{ "name": "voice", "definition": renamed }])),
        |_| {},
    );

    let revision = RuntimeRevision {
        session_id: "session".into(),
        revision: 1,
    };
    let read = running.development(&names(&["voice"]), revision).unwrap();
    assert_eq!(read.definition.modules[0].name.as_deref(), Some("Body"));
    assert_eq!(read.definition.layout.modules["osc"], at(2.0));
    // A move after it lands arranges the definition it adopted.
    running
        .apply_edits(&[move_inside(&["voice"], "amp", 6.0)])
        .unwrap();
    let read = layout_of(&running, &["voice"]);
    assert_eq!((read["osc"], read["amp"]), (at(2.0), at(6.0)));
}

#[test]
fn a_file_arranged_while_a_timed_reload_waits_keeps_the_move_and_takes_the_next() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("voice.json");
    std::fs::write(&file, serde_json::to_vec_pretty(&voice()).unwrap()).unwrap();
    let path = dir.path().join("piece.json");
    let declared = json!([{ "name": "voice", "path": "voice.json" }]);
    std::fs::write(&path, piece(declared.clone()).to_string()).unwrap();
    let document = Invention::from_file(&path.to_string_lossy()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(document).unwrap();
    let mut running = Settled::start(runtime);
    let mut next = piece(declared);
    next["developments"][0]["path"] = json!(file.to_string_lossy());

    land(&mut running, next, |running| {
        running
            .apply_edits(&[move_inside(&["voice"], "osc", 3.0)])
            .unwrap();
    });

    assert_eq!(layout_of(&running, &["voice"])["osc"], at(3.0));
    // What the piece holds is still what the file holds: the next move is
    // written, not refused as changed on disk.
    running
        .apply_edits(&[move_inside(&["voice"], "amp", 4.0)])
        .unwrap();
    let written = Invention::from_file(&file.to_string_lossy()).unwrap();
    assert_eq!(written.layout.modules["osc"], at(3.0));
    assert_eq!(written.layout.modules["amp"], at(4.0));
}

#[test]
fn a_move_made_once_a_timed_reload_landed_starts_from_what_it_brought() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("voice.json");
    std::fs::write(&file, serde_json::to_vec_pretty(&voice()).unwrap()).unwrap();
    let path = dir.path().join("piece.json");
    let declared = json!([{ "name": "voice", "path": "voice.json" }]);
    std::fs::write(&path, piece(declared.clone()).to_string()).unwrap();
    let document = Invention::from_file(&path.to_string_lossy()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(document).unwrap();
    let mut running = Settled::start(runtime);
    // The file is arranged elsewhere, and a timed reload reads it again.
    let mut arranged = voice();
    arranged["layout"] = json!({ "modules": { "amp": { "x": 9.0, "y": 1.0 } } });
    std::fs::write(&file, serde_json::to_vec_pretty(&arranged).unwrap()).unwrap();
    let mut next = piece(declared);
    next["developments"][0]["path"] = json!(file.to_string_lossy());
    next["modules"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "lfo", "type": "lfo" }));
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    let next = Invention::from_json(&next.to_string()).unwrap();
    running.reload_at(next, &when, None).expect("queued");
    // Landed on the audio thread, not yet adopted by anything.
    running.pump.render(64);

    running
        .apply_edits(&[move_inside(&["voice"], "osc", 3.0)])
        .unwrap();

    let written = Invention::from_file(&file.to_string_lossy()).unwrap();
    assert_eq!(
        written.layout.modules["amp"],
        at(9.0),
        "the other arrangement"
    );
    assert_eq!(written.layout.modules["osc"], at(3.0), "the move");
    let read = layout_of(&running, &["voice"]);
    assert_eq!((read["amp"], read["osc"]), (at(9.0), at(3.0)));
}

#[test]
fn a_timed_edit_batch_landing_keeps_a_nested_move_made_while_it_waits() {
    let mut outer = voice();
    outer["developments"] = json!([{ "name": "env", "definition": voice() }]);
    let declared = json!([{ "name": "voice", "definition": outer }]);
    let mut running = start(&piece(declared).to_string());
    let lfo = StructuralEdit::AddModule {
        name: None,
        id: "lfo".into(),
        module_type: "lfo".into(),
        config: Value::Null,
    };
    let when = crate::rpc::EditWhen::AfterSamples { samples: 2048 };
    running.apply_edits_at(&[lfo], &when, None).expect("queued");

    running
        .apply_edits(&[move_inside(&["voice", "env"], "osc", 3.0)])
        .unwrap();
    running.pump.render(64);
    running.live.adopt_landed();

    let document = running.document().unwrap();
    assert!(
        document.modules.iter().any(|spec| spec.id == "lfo"),
        "landed"
    );
    let chain = names(&["voice", "env"]);
    let mut retained = document.clone();
    let inner = crate::invention::reload::inline_definition_mut(&mut retained, &chain).unwrap();
    assert_eq!(inner.layout.modules["osc"], at(3.0));
    assert_eq!(layout_of(&running, &["voice", "env"])["osc"], at(3.0));
}

#[test]
fn a_batch_with_a_file_that_changed_on_disk_moves_nothing_inline_either() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("pad.json");
    std::fs::write(&file, serde_json::to_vec_pretty(&voice()).unwrap()).unwrap();
    let path = dir.path().join("piece.json");
    let declared = json!([
        { "name": "voice", "definition": voice() },
        { "name": "pad", "path": "pad.json" }
    ]);
    std::fs::write(&path, piece(declared).to_string()).unwrap();
    let document = Invention::from_file(&path.to_string_lossy()).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(document).unwrap();
    let mut running = Settled::start(runtime);
    let changed = b"{ \"version\": \"1.0.0\", \"modules\": [], \"connections\": [] }";
    std::fs::write(&file, changed).unwrap();

    let refused = running.apply_edits(&[
        move_inside(&["voice"], "osc", 3.0),
        move_inside(&["pad"], "osc", 4.0),
    ]);

    assert!(refused.is_err());
    assert_eq!(std::fs::read(&file).unwrap(), changed);
    assert!(inline_layout(&running.document().unwrap()).is_empty());
    assert!(layout_of(&running, &["voice"]).is_empty());
    let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(leftovers, 2, "nothing staged is left beside the file");

    // Staged, then refused under the lock by a move of no module: the file
    // is as it was and nothing is left beside it.
    let original = serde_json::to_vec_pretty(&voice()).unwrap();
    std::fs::write(&file, &original).unwrap();
    let nowhere = StructuralEdit::MoveModules {
        development: None,
        positions: [("nowhere".to_string(), Some(at(1.0)))]
            .into_iter()
            .collect(),
    };
    let refused = running.apply_edits(&[move_inside(&["pad"], "osc", 4.0), nowhere]);
    assert!(refused.is_err());
    assert_eq!(std::fs::read(&file).unwrap(), original);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}
