//! An offline render's batch with `set_control` edits authors each value as
//! the daemon does, and reports the configs a host copies into its own
//! document.

use super::RenderEngine;
use crate::rpc::StructuralEdit;
use crate::ControlValue;

const DOCUMENT: &str = r#"{"version":"1.0.0","modules":[
    {"id":"osc","type":"oscillator","config":{"waveform":"sine","frequency":220.0}},
    {"id":"mix","type":"mixer","config":{"channel_count":2,"levels":[0.5,0.5]}},
    {"id":"dac","type":"dac","config":{"soft_clip":false}}],
  "connections":[
    {"from":"osc","from_port":"audio","to":"mix","to_port":"audio.0"},
    {"from":"mix","from_port":"audio_left","to":"dac","to_port":"audio"}]}"#;

fn loaded() -> RenderEngine {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(DOCUMENT).unwrap();
    engine
}

fn edits(json: &str) -> Vec<StructuralEdit> {
    serde_json::from_str(json).unwrap()
}

fn config(engine: &RenderEngine, id: &str) -> serde_json::Value {
    let document = engine.document().unwrap();
    let spec = document.modules.into_iter().find(|spec| spec.id == id);
    spec.unwrap().config
}

#[test]
fn a_reported_config_builds_the_value_again() {
    let engine = loaded();
    let batch = r#"[{"op":"set_control","module_id":"mix","key":"level.1","value":0.25}]"#;
    let report = engine.apply_edits(&edits(batch)).unwrap();
    assert_eq!(report.configs[0].config, config(&engine, "mix"));
    // A host that puts the reported config in its copy, and loads that,
    // hears the value the batch wrote.
    let mut copy: serde_json::Value = serde_json::from_str(DOCUMENT).unwrap();
    copy["modules"][1]["config"] = report.configs[0].config.clone();
    let mut again = RenderEngine::new(48_000);
    again.load_json(&copy.to_string()).unwrap();
    let read = again.get_control("mix", "level.1").unwrap();
    assert_eq!(read, ControlValue::Number(0.25));
}

#[test]
fn a_module_removed_and_added_again_is_built_with_the_batch_value() {
    let engine = loaded();
    let batch = r#"[
        {"op":"remove_module","id":"osc"},
        {"op":"add_module","id":"osc","module_type":"oscillator","config":{"frequency":220.0}},
        {"op":"connect","from":"osc","from_port":"audio","to":"mix","to_port":"audio.0"},
        {"op":"set_control","module_id":"osc","key":"waveform","value":"square"}]"#;
    let report = engine.apply_edits(&edits(batch)).unwrap();
    let read = engine.get_control("osc", "waveform").unwrap();
    assert_eq!(read, ControlValue::String("square".into()));
    assert_eq!(config(&engine, "osc")["waveform"], "square");
    assert_eq!(report.controls_written.len(), 1);
}

#[test]
fn a_report_with_no_failure_leaves_the_field_out() {
    let engine = loaded();
    let batch = r#"[{"op":"set_control","module_id":"osc","key":"frequency","value":330}]"#;
    let report = engine.apply_edits(&edits(batch)).unwrap();
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("controls_failed").is_none());
    assert_eq!(json["controls_written"][0]["key"], "frequency");
}
