//! What an offline edit keeps, as a live install does: a feedback loop the
//! edit leaves alone stays sample-identical to an unedited twin, and a
//! schedule resolved before the edit is refused as stale.

use super::RenderEngine;
use crate::modules::ControlSchedulerControls;

const RATE: u32 = 48_000;

/// A sustained FM feedback loop (`osc1` <-> `osc2`) into the dac, behind an
/// unrelated `lfo` that feeds nothing audible, so removing it shifts every
/// loop module's index; `sched` writes `osc2` on step 1 of `lfo`'s
/// rising edges.
const LOOP: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "lfo", "type": "oscillator", "config": { "frequency": 3.0 } },
        { "id": "osc1", "type": "oscillator", "config": { "frequency": 220.0, "frequency_mod_depth": 300.0 } },
        { "id": "osc2", "type": "oscillator", "config": { "frequency": 331.0, "frequency_mod_depth": 500.0 } },
        { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
            { "at_step": 1, "module": "osc2", "control": "frequency", "value": 330.0 }
        ] } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "osc1", "from_port": "audio", "to": "osc2", "to_port": "frequency_mod" },
        { "from": "osc2", "from_port": "audio", "to": "osc1", "to_port": "frequency_mod" },
        { "from": "osc1", "from_port": "audio", "to": "dac", "to_port": "audio_left" }
    ]
}"#;

fn engine() -> RenderEngine {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(LOOP).unwrap();
    engine
}

fn render(engine: &mut RenderEngine, blocks: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; 2 * 64 * blocks];
    engine.render_interleaved(&mut out).unwrap();
    out
}

#[track_caller]
fn assert_identical(edited: &[f32], twin: &[f32], label: &str) {
    let first = (0..edited.len()).find(|&i| edited[i].to_bits() != twin[i].to_bits());
    assert_eq!(first, None, "{label}");
}

#[test]
fn an_offline_edit_outside_a_feedback_loop_leaves_it_sample_identical() {
    let (mut edited, mut twin) = (engine(), engine());
    assert_identical(
        &render(&mut edited, 5),
        &render(&mut twin, 5),
        "before editing",
    );
    let osc = |frequency: f32| serde_json::json!({ "frequency": frequency });
    edited.add_module("aux", "oscillator", &osc(90.0)).unwrap();
    edited
        .connect("lfo", "audio", "aux", "frequency_mod")
        .unwrap();
    assert_identical(
        &render(&mut edited, 7),
        &render(&mut twin, 7),
        "after adding aux",
    );
    edited.remove_module("lfo").unwrap();
    assert_identical(
        &render(&mut edited, 7),
        &render(&mut twin, 7),
        "after removing lfo",
    );
    edited.add_module("aux", "oscillator", &osc(45.0)).unwrap();
    assert_identical(
        &render(&mut edited, 7),
        &render(&mut twin, 7),
        "after rebuilding aux",
    );
}

/// A schedule write resolved against the directory before an offline edit
/// installed is refused as stale, recording nothing; written afresh, it
/// resolves against what the edit left.
#[test]
fn a_schedule_resolved_before_an_offline_edit_is_refused_as_stale() {
    use crate::invention::declared::{Checked, STALE};
    let mut engine = engine();
    render(&mut engine, 1);
    let surface = engine.control_surfaces.lock().unwrap()["sched"].clone();
    let controls = surface.as_any().unwrap();
    let controls = controls.downcast_ref::<ControlSchedulerControls>().unwrap();
    let generation = controls.declared().generation();
    assert!(generation.is_some(), "offline");
    let config = serde_json::json!({ "frequency": 440.0 });
    engine.add_module("osc2", "oscillator", &config).unwrap();
    let recorded = std::cell::Cell::new(false);
    let checked = Checked {
        generation,
        targets: Vec::new(),
        ramps: 0,
        record: Box::new(|| recorded.set(true)),
    };
    let payload = crate::payload::Payload::new(0_u32);
    let written = controls.declared().write_payload_checked(
        ("schedule", payload, "[]".into()),
        checked,
        &Default::default(),
    );
    assert_eq!(written.map(drop), Err(STALE.to_string()));
    assert!(!recorded.get());
    let schedule =
        r#"[{ "at_step": 1, "module": "osc2", "control": "frequency", "value": 110.0 }]"#;
    engine
        .set_control("sched", "schedule", schedule.into())
        .unwrap();
}
