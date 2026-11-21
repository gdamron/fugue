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
