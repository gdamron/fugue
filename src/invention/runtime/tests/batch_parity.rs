//! An offline render's edit batch refuses a bad edit as the daemon's
//! `ApplyEdits` does: the same edit index, op, reason and message, so an
//! agent reads one refusal whichever engine plays the piece.

use super::*;
use crate::invention::render::RenderEngine;
use crate::rpc::{RpcError, StructuralEdit};

/// A sine through a mixer to the dac.
const DOCUMENT: &str = r#"{"version":"1.0.0","modules":[
    {"id":"osc","type":"oscillator","config":{"waveform":"sine","frequency":220.0}},
    {"id":"mix","type":"mixer","config":{"channel_count":2,"levels":[0.5,0.5]}},
    {"id":"dac","type":"dac","config":{"soft_clip":false}}],
  "connections":[
    {"from":"osc","from_port":"audio","to":"mix","to_port":"audio.0"},
    {"from":"mix","from_port":"audio_left","to":"dac","to_port":"audio"}]}"#;

/// Batches each refused at one edit, by both engines.
const REFUSED: &[&str] = &[
    r#"[{"op":"add_module","id":"osc","module_type":"oscillator"}]"#,
    r#"[{"op":"add_module","id":"osc","module_type":"no_such_type"}]"#,
    r#"[{"op":"add_module","id":"x","module_type":"no_such_type"}]"#,
    r#"[{"op":"add_module","id":"x","module_type":"oscillator","config":{"waveform":"bogus"}}]"#,
    r#"[{"op":"remove_module","id":"nobody"}]"#,
    r#"[{"op":"remove_module","id":"nobody"},
        {"op":"add_module","id":"x","module_type":"no_such_type"}]"#,
    r#"[{"op":"connect","from":"nobody","from_port":"audio","to":"mix","to_port":"audio.1"}]"#,
    r#"[{"op":"connect","from":"osc","from_port":"nowhere","to":"mix","to_port":"audio.1"}]"#,
    r#"[{"op":"connect","from":"osc","from_port":"audio","to":"nobody","to_port":"audio"}]"#,
    r#"[{"op":"connect","from":"osc","from_port":"audio","to":"mix","to_port":"nowhere"}]"#,
    r#"[{"op":"connect","from":"osc","from_port":"audio","to":"mix","to_port":"audio.0"}]"#,
    r#"[{"op":"remove_module","id":"osc"},
        {"op":"connect","from":"osc","from_port":"audio","to":"mix","to_port":"audio.0"}]"#,
    r#"[{"op":"disconnect","from":"nobody","from_port":"audio","to":"mix","to_port":"audio.1"}]"#,
    r#"[{"op":"disconnect","from":"osc","from_port":"audio","to":"mix","to_port":"audio.1"}]"#,
    r#"[{"op":"add_module","id":"","module_type":"oscillator"}]"#,
];

/// What a refusal tells an agent about the edit to blame.
fn blamed(error: &RpcError) -> (usize, String, String, String) {
    let edit = error
        .edit
        .as_deref()
        .unwrap_or_else(|| panic!("no edit named: {error:?}"));
    (
        edit.index,
        edit.op.to_string(),
        format!("{:?}", edit.reason),
        error.message.clone(),
    )
}

#[test]
fn both_engines_refuse_a_bad_edit_alike() {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let mut running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(DOCUMENT).unwrap();
    for batch in REFUSED {
        let edits: Vec<StructuralEdit> = serde_json::from_str(batch).unwrap();
        let live = running.apply_edits(&edits).unwrap_err();
        let offline = engine.apply_edits(&edits).unwrap_err();
        assert_eq!(blamed(&offline), blamed(&live), "{batch}");
        assert_eq!(offline.code, live.code, "{batch}");
    }
}

/// A schedule targeting a module that is not there, which the daemon finds
/// building the edited invention and an offline render installing it.
const UNRESOLVED: &str = r#"[{"op":"add_module","id":"sched","module_type":"control_scheduler",
    "config":{"schedule":[{"at_step":1,"module":"nobody","control":"frequency","value":330.0}]}}]"#;

/// The same, for a schedule targeting a module the batch removes.
const TARGET_REMOVED: &str = r#"[{"op":"add_module","id":"sched","module_type":"control_scheduler",
    "config":{"schedule":[{"at_step":1,"module":"osc","control":"frequency","value":330.0}]}},
    {"op":"remove_module","id":"osc"}]"#;

#[test]
fn both_engines_refuse_a_batch_leaving_a_schedule_unresolved_alike() {
    let invention = Invention::from_json(DOCUMENT).unwrap();
    let (runtime, _) = InventionBuilder::new(48_000).build(invention).unwrap();
    let mut running = runtime
        .start_with_backend(TickBackend::new(48_000))
        .unwrap();
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(DOCUMENT).unwrap();
    let refuse = |batch: &str, running: &mut RunningInvention, engine: &mut RenderEngine| {
        let edits: Vec<StructuralEdit> = serde_json::from_str(batch).unwrap();
        let live = running.apply_edits(&edits).unwrap_err();
        let offline = engine.apply_edits(&edits).unwrap_err();
        assert!(live.edit.is_none() && offline.edit.is_none(), "{batch}");
        assert_eq!(offline.code, live.code, "{batch}");
        (live.message, offline.message)
    };
    let (live, offline) = refuse(UNRESOLVED, &mut running, &mut engine);
    assert_eq!(offline, live);
    let (live, offline) = refuse(TARGET_REMOVED, &mut running, &mut engine);
    // The same scheduler and reason, with the advice an offline change adds.
    let reason = "control_scheduler 'sched': schedule entry at step 1: unknown module 'osc'";
    assert!(live.contains(reason), "{live}");
    assert!(
        offline.contains("'sched'") && offline.contains("unknown module 'osc'"),
        "{offline}"
    );
}
