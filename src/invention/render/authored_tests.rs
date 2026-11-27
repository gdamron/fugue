//! A render's document read with its authored epoch: a call that records an
//! authored write moves the epoch once per write and the document holds
//! it; a performed write or one to an event control records nothing.

use super::RenderEngine;
use crate::{ControlValue, ControlWrite};

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

fn writes(json: &str) -> Vec<ControlWrite> {
    serde_json::from_str(json).unwrap()
}

/// The document's config value for `key` of `id`, as the epoch pairs it.
fn held(engine: &RenderEngine, id: &str, key: &str) -> (serde_json::Value, u64) {
    let (document, epoch) = engine.document_with_epoch();
    let document = document.expect("a loaded render keeps its document");
    let spec = document.modules.into_iter().find(|spec| spec.id == id);
    (spec.unwrap().config[key].clone(), epoch)
}

#[test]
fn an_authored_write_moves_the_epoch_once_and_lands_in_the_document() {
    let engine = loaded();
    let (_, before) = held(&engine, "osc", "frequency");
    let batch = r#"[{"module_id":"osc","key":"frequency","value":330},
                    {"module_id":"osc","key":"waveform","value":"saw","intent":"author"}]"#;
    engine.write_controls(&writes(batch)).unwrap();
    let (frequency, epoch) = held(&engine, "osc", "frequency");
    assert_eq!(frequency.as_f64(), Some(330.0));
    assert_eq!(epoch, before + 2, "once for each authored write");
}

#[test]
fn a_performed_or_event_write_records_nothing() {
    let engine = loaded();
    let (frequency, before) = held(&engine, "osc", "frequency");
    let batch = r#"[{"module_id":"osc","key":"frequency","value":330,"intent":"perform"},
                    {"module_id":"clock","key":"reset","value":true}]"#;
    engine.write_controls(&writes(batch)).unwrap();
    assert_eq!(
        engine.get_control("osc", "frequency").unwrap(),
        ControlValue::Number(330.0)
    );
    assert_eq!(held(&engine, "osc", "frequency"), (frequency, before));
}
