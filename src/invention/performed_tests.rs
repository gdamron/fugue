//! The controls written as performances, read with what they sound, and
//! what the differences make of them against the document.

use crate::invention::render::RenderEngine;
use crate::rpc::{DiffBaseline, DiffQuery, DiffSource, RuntimeRevision, SoundingValue};
use crate::ControlWrite;
use serde_json::json;

const DOCUMENT: &str = r#"{"version":"1.0.0","modules":[
    {"id":"osc","type":"oscillator","config":{"waveform":"sine","frequency":220.0}},
    {"id":"clock","type":"clock","config":{"bpm":120}},
    {"id":"dac","type":"dac","config":{"soft_clip":false}}],
  "connections":[{"from":"osc","from_port":"audio","to":"dac","to_port":"audio"}]}"#;

fn loaded() -> RenderEngine {
    let mut engine = RenderEngine::new(48_000);
    engine.load_json(DOCUMENT).unwrap();
    engine
}

fn write(engine: &RenderEngine, json: serde_json::Value) {
    let writes: Vec<ControlWrite> = serde_json::from_value(json).unwrap();
    engine.write_controls(&writes).unwrap();
}

fn sounding(module_id: &str, key: &str, value: serde_json::Value) -> SoundingValue {
    SoundingValue {
        module_id: module_id.into(),
        key: key.into(),
        value,
    }
}

#[test]
fn a_performed_write_is_listed_with_what_it_sounds_and_others_are_not() {
    let engine = loaded();
    write(
        &engine,
        json!([
            {"module_id":"osc","key":"frequency","value":330,"intent":"perform"},
            {"module_id":"osc","key":"frequency","value":440,"intent":"perform"},
            {"module_id":"clock","key":"reset","value":true,"intent":"perform"},
            {"module_id":"clock","key":"bpm","value":90},
        ]),
    );
    assert_eq!(
        engine.snapshot().performed_values(),
        [sounding("osc", "frequency", json!(440))]
    );
}

#[test]
fn a_module_removed_is_forgotten() {
    let engine = loaded();
    let perform = |module_id: &str, key: &str, value: serde_json::Value| {
        write(
            &engine,
            json!([{"module_id":module_id,"key":key,"value":value,"intent":"perform"}]),
        );
    };
    perform("osc", "frequency", json!(330));
    perform("clock", "bpm", json!(100));
    engine.remove_module("osc").unwrap();
    assert_eq!(
        engine.snapshot().performed_values(),
        [sounding("clock", "bpm", json!(100))]
    );
}

#[test]
fn a_save_would_keep_an_authored_value_written_over_a_performed_one() {
    let engine = loaded();
    let baseline = engine.document().unwrap();
    write(
        &engine,
        json!([{"module_id":"osc","key":"frequency","value":330,"intent":"perform"}]),
    );
    let read = |engine: &RenderEngine| {
        let current = engine.document().unwrap();
        let sounding = engine.snapshot().performed_values();
        let revision = RuntimeRevision {
            session_id: "render".into(),
            revision: 1,
        };
        let source = DiffSource {
            baseline: (&baseline, DiffBaseline::Loaded),
            current: &current,
            sounding: &sounding,
            revision: &revision,
        };
        source.page(&DiffQuery::default()).unwrap()
    };
    let page = read(&engine);
    assert!(page.changes.is_empty());
    assert_eq!(
        serde_json::to_value(&page.performed).unwrap(),
        json!([{"module_id":"osc","key":"frequency","document":220.0,"sounding":330}])
    );
    write(
        &engine,
        json!([{"module_id":"osc","key":"frequency","value":330}]),
    );
    let page = read(&engine);
    assert!(page.performed.is_empty(), "{page:?}");
    assert_eq!(page.total_changes, 1);
}
