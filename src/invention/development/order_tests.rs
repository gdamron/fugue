//! A development runs an inner scheduler before the modules it writes, as
//! the top level does, so a scheduled write lands in the block it falls
//! in; an offline `schedule` write that retargets an inner scheduler
//! orders the inner modules again, so it plays as loaded.

use super::compiled_graph::process_order;
use crate::invention::RenderEngine;
use crate::ControlValue;

#[test]
fn routes_alone_order_as_before() {
    // No ordering edges: reverse post-order of the routes.
    assert_eq!(
        process_order(3, &[(0, 1)], &[vec![], vec![], vec![]]),
        (vec![2, 0, 1], false)
    );
}

#[test]
fn a_scheduler_runs_before_the_module_it_writes() {
    // Module 1 writes module 0, which routing alone puts after it.
    let (order, cycle) = process_order(2, &[], &[vec![], vec![0]]);
    assert_eq!((order, cycle), (vec![1, 0], false));
}

#[test]
fn a_write_upstream_of_its_scheduler_is_no_loop() {
    // 0 (a clock) drives 1 (a scheduler) writing 0's tempo: the ordering
    // edge would close a loop and is left out, so the development keeps
    // its block path and the route's order.
    let (order, cycle) = process_order(2, &[(0, 1)], &[vec![], vec![0]]);
    assert_eq!((order, cycle), (vec![0, 1], false));
    // Nor does it turn a later route into a loop.
    let (order, cycle) = process_order(2, &[(1, 0)], &[vec![1], vec![]]);
    assert_eq!((order, cycle), (vec![1, 0], false));
}

#[test]
fn a_routed_loop_is_still_one() {
    let (_, cycle) = process_order(3, &[(0, 1), (1, 0)], &[vec![], vec![], vec![0, 1]]);
    assert!(cycle);
}

const RATE: u32 = 48_000;

/// A clock at 97 bpm stepping a development whose scheduler writes
/// `schedule`; its inner `o` is the audio. Loaded writing `m`, the inner
/// order puts `o` before the scheduler; writing `o`, after it.
fn document(schedule: &str) -> String {
    format!(
        r#"{{
        "version": "1.0.0",
        "developments": [{{ "name": "pattern", "definition": {{
            "version": "1.0.0",
            "modules": [
                {{ "id": "sched", "type": "control_scheduler", "config": {{ "schedule": {schedule} }} }},
                {{ "id": "m", "type": "oscillator", "config": {{ "frequency": 110.0 }} }},
                {{ "id": "o", "type": "oscillator", "config": {{ "frequency": 440.0 }} }}
            ],
            "connections": [],
            "inputs": [{{ "name": "clock", "to": "sched", "to_port": "clock" }}],
            "outputs": [{{ "name": "audio", "from": "o", "from_port": "audio" }}],
            "controls": [{{ "name": "schedule", "module": "sched", "control": "schedule" }}]
        }} }}],
        "modules": [
            {{ "id": "clock", "type": "clock", "config": {{ "bpm": 97.0 }} }},
            {{ "id": "p", "type": "pattern" }},
            {{ "id": "dac", "type": "dac", "config": {{ "soft_clip": false }} }}
        ],
        "connections": [
            {{ "from": "clock", "from_port": "beat", "to": "p", "to_port": "clock" }},
            {{ "from": "p", "from_port": "audio", "to": "dac", "to_port": "audio" }}
        ]
    }}"#
    )
}

const OLD: &str = r#"[{"at_step":6,"module":"m","control":"frequency","value":220.0}]"#;
const NEW: &str = r#"[{"at_step":6,"module":"o","control":"frequency","value":330.0}]"#;

/// Renders `doc` offline for five seconds, writing `write` to the
/// development's `schedule` after one.
fn render(doc: &str, write: Option<&str>) -> Vec<f32> {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(doc).unwrap();
    let mut out = vec![0.0f32; RATE as usize * 2];
    engine.render_interleaved(&mut out).unwrap();
    if let Some(schedule) = write {
        let schedule = ControlValue::String(schedule.into());
        engine.set_control("p", "schedule", schedule).unwrap();
    }
    let mut rest = vec![0.0f32; RATE as usize * 8];
    engine.render_interleaved(&mut rest).unwrap();
    out.extend(rest);
    out
}

#[test]
fn an_offline_inner_retarget_plays_as_loaded() {
    let loaded = render(&document(NEW), None);
    let written = render(&document(OLD), Some(NEW));
    let first = loaded
        .iter()
        .zip(&written)
        .position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(first, None);
    // The write is heard: the oscillator changes pitch at step 6.
    assert_ne!(loaded, render(&document(OLD), None));
}
