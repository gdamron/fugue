//! A live session that edits a step sequencer's `pattern` (a payload
//! control) beside scalar writes logs each pattern write as the text it
//! reads back, exports it, and replays it sample for sample, writing the
//! text again through the control's front door. Scalar entries serialize
//! as they always have.

use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::rpc::ControlWriteIntent;
use crate::{
    ControlValue, Invention, InventionBuilder, LoggedRequest, LoggedValue, RenderEngine, RequestLog,
};

const DOC: &str = include_str!("../../../../examples/step_sequencer.json");

/// The device's callback size, which is also the graph's block size.
const BLOCK: usize = 64;

const RISING: &str = r#"[{"note":0},{"note":3},{"note":7},{"note":12,"gate_length":0.9}]"#;
const FALLING: &str = r#" [ {"note": 12}, {"note": null}, {"note": 5}, {"note": 0} ] "#;

/// A live session of [`DOC`]: a scalar write, a performed pattern write,
/// another scalar, an authored pattern write. Returns the exported log,
/// through JSON, and the left channel as played.
fn live_session() -> (RequestLog, Vec<f32>) {
    let invention = Invention::from_json(DOC).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    let snapshot = running.snapshot();
    let mut played = pump.render(100);
    let write = |key: &str, value: ControlValue, intent| {
        let module = if key == "waveform" { "osc" } else { "seq" };
        (snapshot.set_control_with_intent(module, key, value, intent)).unwrap();
    };
    write("waveform", "square".into(), ControlWriteIntent::Perform);
    played.extend(pump.render(150));
    write("pattern", RISING.into(), ControlWriteIntent::Perform);
    played.extend(pump.render(300));
    write("gate_length", 0.25.into(), ControlWriteIntent::Author);
    played.extend(pump.render(200));
    write("pattern", FALLING.into(), ControlWriteIntent::Author);
    played.extend(pump.render(600));
    let log = running.request_log();
    let json = serde_json::to_string(&log).unwrap();
    running.stop();
    (serde_json::from_str(&json).unwrap(), played)
}

/// An offline render of [`DOC`] as loaded, in device-sized calls, replaying
/// `log`; with how many replayed requests it refused.
fn offline(log: &RequestLog, frames: usize) -> (Vec<f32>, usize) {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(DOC).unwrap();
    engine.replay(log).unwrap();
    let mut left = Vec::with_capacity(frames);
    let mut buffer = [0.0f32; BLOCK * 2];
    for _ in 0..frames / BLOCK {
        engine.render_interleaved(&mut buffer).unwrap();
        left.extend(buffer.chunks_exact(2).map(|frame| frame[0]));
    }
    (left, engine.replay_refused())
}

fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|sample| sample.to_bits()).collect()
}

#[test]
fn a_session_with_pattern_writes_logs_them_and_replays_sample_for_sample() {
    let (log, played) = live_session();
    assert!(log.is_exact(), "{log:?}");
    assert_eq!(log.unlogged, 0);
    let values: Vec<_> = log.requests.iter().map(|r| (&*r.key, &r.value)).collect();
    let text = |text: &str| LoggedValue::Text(text.into());
    // Each pattern as it reads back: canonical, whatever the spacing written.
    let rising = text(r#"[{"note":0},{"note":3},{"note":7},{"note":12,"gate_length":0.9}]"#);
    let falling = text(r#"[{"note":12},{"note":null},{"note":5},{"note":0}]"#);
    assert_eq!(
        values,
        [
            ("waveform", &LoggedValue::Index(1)),
            ("pattern", &rising),
            ("gate_length", &LoggedValue::Number(0.25)),
            ("pattern", &falling),
        ]
    );

    let (replayed, refused) = offline(&log, played.len());
    assert_eq!(refused, 0);
    assert_eq!(bits(&played), bits(&replayed), "sample for sample");

    // The pattern writes are what make it match.
    let mut scalars = log.clone();
    scalars.requests.retain(|r| r.key != "pattern");
    let (without, _) = offline(&scalars, played.len());
    assert_ne!(bits(&played), bits(&without));
}

/// A logged pattern its control now refuses counts refused, and the render
/// carries on.
#[test]
fn a_replayed_pattern_the_control_refuses_counts_refused() {
    let (mut log, played) = live_session();
    let pattern = log.requests.iter_mut().find(|r| r.key == "pattern");
    pattern.unwrap().value = LoggedValue::Text("[1, ".into());
    let (_, refused) = offline(&log, played.len());
    assert_eq!(refused, 1);
}

/// Scalar entries serialize exactly as before payloads were logged; a
/// payload adds one tagged variant beside them.
#[test]
fn scalar_entries_serialize_as_before_and_text_beside_them() {
    let entry = |value| LoggedRequest {
        sample: 64,
        module_id: "seq".into(),
        key: "k".into(),
        value,
        due: None,
        beat: None,
    };
    let json = |value| serde_json::to_string(&entry(value)).unwrap();
    let head = r#"{"sample":64,"module_id":"seq","key":"k","value":"#;
    assert_eq!(
        json(LoggedValue::Number(0.25)),
        format!(r#"{head}{{"type":"number","value":0.25}}}}"#)
    );
    assert_eq!(
        json(LoggedValue::Integer(-3)),
        format!(r#"{head}{{"type":"integer","value":-3}}}}"#)
    );
    assert_eq!(
        json(LoggedValue::Index(2)),
        format!(r#"{head}{{"type":"index","value":2}}}}"#)
    );
    assert_eq!(
        json(LoggedValue::Bool(true)),
        format!(r#"{head}{{"type":"bool","value":true}}}}"#)
    );
    let text = LoggedValue::Text(r#"[{"note":0}]"#.into());
    assert_eq!(
        json(text.clone()),
        format!(r#"{head}{{"type":"text","value":"[{{\"note\":0}}]"}}}}"#)
    );
    let back: LoggedRequest = serde_json::from_str(&json(text.clone())).unwrap();
    assert_eq!(back.value, text);
}
