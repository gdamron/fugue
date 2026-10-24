//! A development key reaching both a declared control and a legacy one: a
//! live write to it reaches the legacy alias without a request, so the log
//! is not exact, and its replay writes the legacy alias as live did once
//! the declared part applies.

use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::{ControlValue, Invention, InventionBuilder, RenderEngine};

const BLOCK: usize = 64;

/// Development `inner`, whose `mix` reaches a vca's level and a (legacy)
/// lfo's rate, run as module `n` to the dac.
fn document() -> String {
    let inner = serde_json::json!({
        "version": "1.0.0",
        "modules": [{ "id": "a", "type": "vca" }, { "id": "l", "type": "lfo" }],
        "connections": [],
        "outputs": [{ "name": "a", "from": "a", "from_port": "audio" }],
        "controls": [
            { "name": "mix", "module": "a", "control": "level" },
            { "name": "mix", "module": "l", "control": "rate" },
            { "name": "rate", "module": "l", "control": "rate" }
        ]
    });
    serde_json::json!({
        "version": "1.0.0",
        "developments": [{ "name": "inner", "definition": inner }],
        "modules": [
            { "id": "n", "type": "inner" },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "n", "from_port": "a", "to": "dac", "to_port": "audio_left" }
        ]
    })
    .to_string()
}

#[test]
fn a_replayed_write_to_a_mixed_key_writes_its_legacy_alias() {
    let doc = document();
    let invention = Invention::from_json(&doc).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    pump.render(10);
    running.set_control("n", "mix", 0.75.into()).unwrap();
    pump.render(10);
    assert_eq!(
        running.get_control("n", "rate").unwrap(),
        ControlValue::from(0.75)
    );
    let log = running.request_log();
    running.stop();
    assert_eq!(log.unlogged, 1, "the legacy alias took the write unlogged");
    assert!(!log.is_exact());

    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(&doc).unwrap();
    engine.replay(&log).unwrap();
    let rate = |engine: &RenderEngine| engine.get_control("n", "rate").unwrap();
    assert_ne!(rate(&engine), ControlValue::from(0.75), "not yet due");
    let mut buffer = [0.0f32; BLOCK * 2];
    for _ in 0..20 {
        engine.render_interleaved(&mut buffer).unwrap();
    }
    assert_eq!(engine.replay_refused(), 0);
    assert_eq!(rate(&engine), ControlValue::from(0.75), "the lfo follows");
}
