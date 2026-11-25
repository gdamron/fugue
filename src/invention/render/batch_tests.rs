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
    engine.apply_edits(&edits(ADD_VOICE)).unwrap();
    let (modules, connections) = shape(&engine);
    assert!(modules.contains(&"osc2".to_string()));
    assert!(connections.contains(&"osc2.audio>mix.audio.1".to_string()));
}

/// Frames that hold no whole number of the 220 Hz tone's cycles, so a
/// restarted oscillator would be heard out of phase.
const FRAMES: usize = 1000;

#[test]
fn survivors_keep_playing_through_an_edit() {
    // The same piece rendered straight through, and with a voice added
    // and removed again in between: osc never restarts, so what it plays
    // before and after the edits is the same, sample for sample.
    let mut kept = loaded();
    let mut edited = loaded();
    let before = render(&mut kept, FRAMES);
    assert_eq!(render(&mut edited, FRAMES), before);
    edited.apply_edits(&edits(ADD_VOICE)).unwrap();
    let removed = r#"[{"op":"remove_module","id":"osc2"}]"#;
    edited.apply_edits(&edits(removed)).unwrap();
    assert_eq!(render(&mut edited, FRAMES), render(&mut kept, FRAMES));
}

#[test]
fn survivors_keep_playing_through_one_batch_that_adds_and_removes() {
    let mut kept = loaded();
    let mut edited = loaded();
    render(&mut kept, FRAMES);
    render(&mut edited, FRAMES);
    let batch = r#"[
        {"op":"add_module","id":"osc2","module_type":"oscillator"},
        {"op":"connect","from":"osc2","from_port":"audio","to":"mix","to_port":"audio.1"},
        {"op":"remove_module","id":"osc2"}]"#;
    edited.apply_edits(&edits(batch)).unwrap();
    assert_eq!(shape(&edited), shape(&kept));
    assert_eq!(render(&mut edited, FRAMES), render(&mut kept, FRAMES));
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
            r#"[{"op":"connect","from":"osc","from_port":"nowhere","to":"mix","to_port":"audio.1"}]"#,
            EditFailureReason::UnknownPort,
        ),
        (
            r#"[{"op":"disconnect","from":"osc","from_port":"audio","to":"mix","to_port":"audio.1"}]"#,
            EditFailureReason::ConnectionNotFound,
        ),
        (
            r#"[{"op":"disconnect","from":"nobody","from_port":"audio","to":"mix","to_port":"audio.1"}]"#,
            EditFailureReason::UnknownModule,
        ),
        // An existing id is a duplicate before its type is looked up.
        (
            r#"[{"op":"add_module","id":"osc","module_type":"no_such_type"}]"#,
            EditFailureReason::DuplicateModule,
        ),
        (
            r#"[{"op":"add_module","id":"","module_type":"oscillator"}]"#,
            EditFailureReason::InvalidName,
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
    let frequency = engine.get_control("osc", "frequency").unwrap();
    assert_eq!(frequency.as_number(), Ok(440.0));
}

#[test]
fn the_first_edit_that_cannot_apply_is_named_whatever_follows() {
    // Each later edit would fail too, and builds before the graph is
    // touched, but the refusal names the first.
    let cases = [
        (
            r#"[{"op":"remove_module","id":"nobody"},
                {"op":"add_module","id":"x","module_type":"no_such_type"}]"#,
            (0, EditFailureReason::UnknownModule),
        ),
        (
            r#"[{"op":"remove_module","id":"osc"},
                {"op":"connect","from":"osc","from_port":"audio","to":"mix","to_port":"audio.0"}]"#,
            (1, EditFailureReason::UnknownModule),
        ),
        (
            r#"[{"op":"add_module","id":"osc2","module_type":"oscillator"},
                {"op":"add_module","id":"osc2","module_type":"oscillator","config":{"waveform":"bogus"}}]"#,
            (1, EditFailureReason::DuplicateModule),
        ),
    ];
    for (batch, expected) in cases {
        let engine = loaded();
        let before = shape(&engine);
        let error = engine.apply_edits(&edits(batch)).unwrap_err();
        let failure = error.edit.as_deref().expect("names the failing edit");
        assert_eq!(
            (failure.index, failure.reason),
            expected,
            "{}",
            error.message
        );
        assert_eq!(shape(&engine), before, "{batch}");
    }
}

#[test]
fn a_batch_refused_after_every_edit_applied_changes_nothing() {
    // Each edit applies in turn, but the change as a whole cannot be
    // prepared: the scheduler it adds targets the module it removes.
    let mut engine = loaded();
    let mut reference = loaded();
    let before = shape(&engine);
    let batch = r#"[
        {"op":"add_module","id":"sched","module_type":"control_scheduler",
         "config":{"schedule":[{"at_step":1,"module":"osc","control":"frequency","value":330.0}]}},
        {"op":"remove_module","id":"osc"}]"#;
    let error = engine.apply_edits(&edits(batch)).unwrap_err();
    assert!(error.edit.is_none(), "{error:?}");
    assert!(
        error.message.contains("nothing was applied"),
        "{}",
        error.message
    );
    assert_eq!(shape(&engine), before);
    assert_eq!(render(&mut engine, FRAMES), render(&mut reference, FRAMES));
}

#[test]
fn an_oversized_batch_is_refused() {
    let one = r#"{"op":"remove_module","id":"nobody"}"#;
    let batch = format!(
        "[{}]",
        vec![one; crate::rpc::MAX_EDITS_PER_BATCH + 1].join(",")
    );
    let error = loaded().apply_edits(&edits(&batch)).unwrap_err();
    assert!(error.edit.is_none(), "{error:?}");
}

#[test]
fn presentation_edits_are_refused_before_anything_changes() {
    let engine = loaded();
    let before = shape(&engine);
    let batch = r#"[
        {"op":"add_module","id":"osc2","module_type":"oscillator"},
        {"op":"name_modules","names":{"osc":"Lead"}}]"#;
    let error = engine.apply_edits(&edits(batch)).unwrap_err();
    assert!(
        error.message.starts_with("edit 1 (name_modules) refused"),
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
