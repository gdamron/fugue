//! A batch of edits on an offline render applies whole, in order, without
//! restarting what survives, or is refused with nothing changed, naming the
//! edit that failed.

use super::RenderEngine;
use crate::rpc::{EditFailureReason, EditOp, StructuralEdit};

const RATE: u32 = 48_000;

/// A sine through a mixer to the dac.
const DOCUMENT: &str = r#"{"version":"1.0.0","modules":[
    {"id":"osc","type":"oscillator","config":{"waveform":"sine","frequency":220.0}},
    {"id":"mix","type":"mixer","config":{"channel_count":2,"levels":[0.5,0.5]}},
    {"id":"dac","type":"dac","config":{"soft_clip":false}}],
  "connections":[
    {"from":"osc","from_port":"audio","to":"mix","to_port":"audio.0"},
    {"from":"mix","from_port":"audio_left","to":"dac","to_port":"audio"}]}"#;

fn loaded() -> RenderEngine {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(DOCUMENT).unwrap();
    engine
}

fn edits(json: &str) -> Vec<StructuralEdit> {
    serde_json::from_str(json).unwrap()
}

fn shape(engine: &RenderEngine) -> (Vec<String>, Vec<String>) {
    let modules = engine.list_modules().into_iter().map(|m| m.id).collect();
    let connections = engine
        .list_connections()
        .into_iter()
        .map(|c| format!("{}.{}>{}.{}", c.from, c.from_port, c.to, c.to_port))
        .collect::<Vec<_>>();
    let mut connections = connections;
    connections.sort();
    (modules, connections)
}

fn render(engine: &mut RenderEngine, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames * 2];
    engine.render_interleaved(&mut out).unwrap();
    out
}

const ADD_VOICE: &str = r#"[
    {"op":"add_module","id":"osc2","module_type":"oscillator","config":{"waveform":"sine","frequency":330.0}},
    {"op":"connect","from":"osc2","from_port":"audio","to":"mix","to_port":"audio.1"}]"#;

#[test]
fn a_batch_adds_and_wires_a_module_in_order() {
    let engine = loaded();
    assert_eq!(engine.apply_edits(&edits(ADD_VOICE)).unwrap(), 2);
    let (modules, connections) = shape(&engine);
    assert!(modules.contains(&"osc2".to_string()));
    assert!(connections.contains(&"osc2.audio>mix.audio.1".to_string()));
}

#[test]
fn survivors_keep_playing_through_an_edit() {
    // The same piece rendered straight through, and with a voice added
    // and removed again in between: osc never restarts, so what it plays
    // before and after the edits is the same, sample for sample.
    let mut kept = loaded();
    let mut edited = loaded();
    let before = render(&mut kept, 4800);
    assert_eq!(render(&mut edited, 4800), before);
    edited.apply_edits(&edits(ADD_VOICE)).unwrap();
    let removed = r#"[{"op":"remove_module","id":"osc2"}]"#;
    edited.apply_edits(&edits(removed)).unwrap();
    assert_eq!(render(&mut edited, 4800), render(&mut kept, 4800));
}

#[test]
fn a_batch_whose_last_edit_fails_changes_nothing() {
    let mut engine = loaded();
    let mut reference = loaded();
    let before = shape(&engine);
    let batch = r#"[
        {"op":"add_module","id":"osc2","module_type":"oscillator"},
        {"op":"connect","from":"osc2","from_port":"audio","to":"mix","to_port":"audio.1"},
        {"op":"remove_module","id":"osc"},
        {"op":"connect","from":"mix","from_port":"audio_left","to":"dac","to_port":"no_such_port"}]"#;
    let error = engine.apply_edits(&edits(batch)).unwrap_err();
    let failure = error.edit.as_deref().expect("names the failing edit");
    assert_eq!((failure.index, failure.op), (3, EditOp::Connect));
    assert_eq!(failure.reason, EditFailureReason::UnknownPort);
    assert!(
        error.message.starts_with("edit 3 (connect) refused"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("nothing was applied"),
        "{}",
        error.message
    );
    assert_eq!(shape(&engine), before);
    // And the sound is as it was.
    assert_eq!(render(&mut engine, 4800), render(&mut reference, 4800));
}

#[test]
fn each_check_names_its_edit() {
    let cases = [
        (
            r#"[{"op":"add_module","id":"osc","module_type":"oscillator"}]"#,
            EditFailureReason::DuplicateModule,
        ),
        (
            r#"[{"op":"add_module","id":"x","module_type":"no_such_type"}]"#,
            EditFailureReason::UnknownModuleType,
        ),
        (
            r#"[{"op":"add_module","id":"x","module_type":"oscillator","config":{"waveform":"bogus"}}]"#,
            EditFailureReason::InvalidConfig,
        ),
        (
            r#"[{"op":"remove_module","id":"nobody"}]"#,
            EditFailureReason::UnknownModule,
        ),
        (
            r#"[{"op":"connect","from":"osc","from_port":"audio","to":"mix","to_port":"audio.0"}]"#,
            EditFailureReason::ConnectionExists,
        ),
        (
            r#"[{"op":"connect","from":"nobody","from_port":"audio","to":"mix","to_port":"audio.1"}]"#,
            EditFailureReason::UnknownModule,
        ),
        (
            r#"[{"op":"disconnect","from":"osc","from_port":"audio","to":"mix","to_port":"audio.1"}]"#,
            EditFailureReason::ConnectionNotFound,
        ),
    ];
    for (batch, expected) in cases {
        let engine = loaded();
        let before = shape(&engine);
        let error = engine.apply_edits(&edits(batch)).unwrap_err();
        let failure = error
            .edit
            .as_deref()
            .unwrap_or_else(|| panic!("{batch}: {}", error.message));
        assert_eq!(
            (failure.index, failure.reason),
            (0, expected),
            "{batch}: {}",
            error.message
        );
        assert_eq!(shape(&engine), before, "{batch}");
    }
}

#[test]
fn a_module_removed_and_added_again_in_one_batch_is_replaced() {
    let engine = loaded();
    let batch = r#"[
        {"op":"remove_module","id":"osc"},
        {"op":"add_module","id":"osc","module_type":"oscillator","config":{"frequency":440.0}},
        {"op":"connect","from":"osc","from_port":"audio","to":"mix","to_port":"audio.0"}]"#;
    engine.apply_edits(&edits(batch)).unwrap();
    assert_eq!(shape(&engine), shape(&loaded()));
}

#[test]
fn document_edits_are_refused_before_anything_changes() {
    let engine = loaded();
    let before = shape(&engine);
    let batch = r#"[
        {"op":"add_module","id":"osc2","module_type":"oscillator"},
        {"op":"set_control","module_id":"osc","key":"frequency","value":440.0}]"#;
    let error = engine.apply_edits(&edits(batch)).unwrap_err();
    assert!(
        error.message.starts_with("edit 1 (set_control) refused"),
        "{}",
        error.message
    );
    assert_eq!(shape(&engine), before);
}

#[test]
fn an_empty_batch_and_an_unloaded_engine_are_refused() {
    assert!(loaded().apply_edits(&[]).is_err());
    let engine = RenderEngine::new(RATE);
    let error = engine.apply_edits(&edits(ADD_VOICE)).unwrap_err();
    assert!(
        error.message.contains("no invention loaded"),
        "{}",
        error.message
    );
}
