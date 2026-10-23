//! An offline edit moves what waits on the render's request drain with the
//! modules it targets, as a live install does: a request queued before the
//! edit reaches its own module at its new index, and one whose module the
//! edit removed or rebuilt is refused rather than reaching whatever took
//! its place.

use super::RenderEngine;
use crate::control_request::{ControlTarget, Request, RequestValue, RtValue};
use crate::ControlValue;

const RATE: u32 = 48_000;

/// Four unconnected oscillators at 440 Hz ahead of the dac, so removing
/// the first shifts the others down one index.
const DOCUMENT: &str = r#"{"version":"1.0.0","modules":[
    {"id":"m0","type":"oscillator","config":{"frequency":440.0}},
    {"id":"m1","type":"oscillator","config":{"frequency":440.0}},
    {"id":"m2","type":"oscillator","config":{"frequency":440.0}},
    {"id":"m3","type":"oscillator","config":{"frequency":440.0}},
    {"id":"dac","type":"dac","config":{"soft_clip":false}}
],"connections":[]}"#;

fn frequency(engine: &RenderEngine, id: &str) -> f32 {
    match engine.get_control(id, "frequency").unwrap() {
        ControlValue::Number(hz) => hz,
        other => panic!("{other:?}"),
    }
}

/// Queues a write of 220 Hz to `m2`'s frequency on the render's drain, due a
/// block from now, as resolved against the order now playing.
fn queue_for_m2(engine: &RenderEngine) {
    let control = {
        let surfaces = engine.control_surfaces.lock().unwrap();
        surfaces["m2"].declaration("frequency").unwrap().index
    };
    let mut graph = engine.graph.as_ref().unwrap().lock().unwrap();
    let module_idx = graph.modules.get_index_of("m2").unwrap();
    let at = graph.current_sample + 64;
    let drain = graph.requests.as_mut().unwrap();
    let target = ControlTarget {
        // No edit installed yet: the render's first generation.
        generation: 0,
        module_idx,
        control,
    };
    let request = Request::new(target, RequestValue::Value(RtValue::F32(220.0)));
    drain.pending.insert(request, at);
}

fn engine() -> RenderEngine {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(DOCUMENT).unwrap();
    let mut out = vec![0.0f32; 512];
    engine.render_interleaved(&mut out).unwrap();
    engine
}

fn render(engine: &mut RenderEngine) {
    let mut out = vec![0.0f32; 1024];
    engine.render_interleaved(&mut out).unwrap();
}

#[test]
fn a_request_queued_before_an_offline_edit_follows_its_module() {
    let mut engine = engine();
    queue_for_m2(&engine);
    engine.remove_module("m0").unwrap();
    render(&mut engine);
    assert_eq!(frequency(&engine, "m2"), 220.0);
    assert_eq!(
        frequency(&engine, "m3"),
        440.0,
        "reached the module now at m2's index"
    );
}

#[test]
fn a_request_whose_module_an_offline_edit_rebuilds_or_removes_is_refused() {
    let config = serde_json::json!({ "frequency": 440.0 });
    for rebuild in [false, true] {
        let mut engine = engine();
        queue_for_m2(&engine);
        if rebuild {
            engine.add_module("m2", "oscillator", &config).unwrap();
            engine.remove_module("m0").unwrap();
        } else {
            engine.remove_module("m2").unwrap();
        }
        render(&mut engine);
        for id in ["m1", "m3"] {
            assert_eq!(frequency(&engine, id), 440.0, "{id}, rebuild {rebuild}");
        }
        if rebuild {
            assert_eq!(
                frequency(&engine, "m2"),
                440.0,
                "reached the rebuilt instance"
            );
        }
    }
}
