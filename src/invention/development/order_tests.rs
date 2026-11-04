//! A development runs an inner scheduler before the modules it writes, as
//! the top level does, so a scheduled write lands in the block it falls
//! in; an offline `schedule` write that retargets an inner scheduler
//! orders the inner modules again, so it plays as loaded.

use super::compiled_graph::{group_sequence, process_order, Grouping};
use crate::invention::RenderEngine;
use crate::ControlValue;

#[test]
fn routes_alone_order_as_before() {
    // No ordering edges: reverse post-order of the routes.
    assert_eq!(
        process_order(3, &[(0, 1)], &[vec![], vec![], vec![]]),
        vec![2, 0, 1]
    );
}

#[test]
fn a_scheduler_runs_before_the_module_it_writes() {
    // Module 1 writes module 0, which routing alone puts after it.
    assert_eq!(process_order(2, &[], &[vec![], vec![0]]), vec![1, 0]);
}

#[test]
fn a_write_upstream_of_its_scheduler_is_no_loop() {
    // 0 (a clock) drives 1 (a scheduler) writing 0's tempo: the ordering
    // edge would close a loop and is left out, so neither joins a feedback
    // group and the route's order stays.
    assert_eq!(process_order(2, &[(0, 1)], &[vec![], vec![0]]), vec![0, 1]);
    assert!(Grouping::of(2, &[(0, 1)], &[])
        .groups
        .iter()
        .all(|group| !group.feedback));
    // Nor does it turn a later route into a loop.
    assert_eq!(process_order(2, &[(1, 0)], &[vec![1], vec![]]), vec![1, 0]);
}

#[test]
fn a_routed_loop_is_one_feedback_group_and_the_rest_run_alone() {
    // 0 feeds the loop 1 ↔ 2, which feeds 3.
    let grouping = Grouping::of(4, &[(0, 1), (1, 2), (2, 1), (2, 3)], &[]);
    let feedback: Vec<_> = grouping.groups.iter().filter(|g| g.feedback).collect();
    assert_eq!(feedback.len(), 1);
    assert_eq!(feedback[0].members, vec![1, 2]);
    assert_eq!(grouping.groups.len(), 3);
    // A self-loop is a feedback group of one.
    let grouping = Grouping::of(2, &[(0, 1), (1, 1)], &[]);
    assert!(grouping.groups[grouping.group_of[1]].feedback);
    assert!(!grouping.groups[grouping.group_of[0]].feedback);
}

#[test]
fn an_ordering_edge_into_a_loop_orders_groups_not_members() {
    // 1 and 2 feed each other; 0 writes 2. Routing alone delays 2 → 1;
    // the write runs 0 first but leaves the loop's members, and which
    // route is delayed, as routing put them.
    let routes = [(1, 2), (2, 1)];
    let grouping = Grouping::of(3, &routes, &[]);
    let loop_group = grouping.group_of[1];
    assert_eq!(grouping.groups[loop_group].members, vec![1, 2]);
    let order = process_order(3, &routes, &[vec![2], vec![], vec![]]);
    let sequence = group_sequence(&order, &grouping.group_of, grouping.groups.len());
    assert_eq!(sequence, vec![grouping.group_of[0], loop_group]);
}

#[test]
fn every_group_follows_what_feeds_it() {
    // 0 → 1 ↔ 2 → 3, with 4 → 2 and 0 writing 4.
    let routes = [(0, 1), (1, 2), (2, 1), (2, 3), (4, 2)];
    let targets = [vec![4], vec![], vec![], vec![], vec![]];
    let grouping = Grouping::of(5, &routes, &[]);
    let order = process_order(5, &routes, &targets);
    let sequence = group_sequence(&order, &grouping.group_of, grouping.groups.len());
    let at = |module: usize| {
        sequence
            .iter()
            .position(|&g| g == grouping.group_of[module])
            .unwrap()
    };
    assert_eq!(sequence.len(), grouping.groups.len());
    assert!(at(0) < at(1) && at(0) < at(4) && at(4) < at(2) && at(2) < at(3));
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

/// `document`'s development wrapped in another, `shell`, which exposes the
/// inner one's `schedule` as its own.
fn nested(schedule: &str) -> String {
    let doc: serde_json::Value = serde_json::from_str(&document(schedule)).unwrap();
    let pattern = doc["developments"][0].clone();
    let shell = serde_json::json!({ "name": "shell", "definition": {
        "version": "1.0.0",
        "modules": [{ "id": "inner", "type": "pattern" }],
        "connections": [],
        "inputs": [{ "name": "clock", "to": "inner", "to_port": "clock" }],
        "outputs": [{ "name": "audio", "from": "inner", "from_port": "audio" }],
        "controls": [{ "name": "schedule", "module": "inner", "control": "schedule" }]
    } });
    let mut doc = doc;
    doc["developments"] = serde_json::json!([pattern, shell]);
    doc["modules"][1]["type"] = "shell".into();
    doc.to_string()
}

#[test]
fn a_retarget_through_nested_developments_plays_as_loaded() {
    let loaded = render(&nested(NEW), None);
    let written = render(&nested(OLD), Some(NEW));
    let first = loaded
        .iter()
        .zip(&written)
        .position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(first, None);
    assert_eq!(loaded, render(&document(NEW), None), "as one development");
}

/// `OLD` at another step, writing `m` as `OLD` does.
const LATER: &str = r#"[{"at_step":7,"module":"m","control":"frequency","value":220.0}]"#;

/// A script's offline `schedule` write losing to a user's at one sample
/// (as it is applied, when `overtaken` by a replay after it was ranked)
/// leaves the user's playing and read back, the development's record of
/// it included, so the render matches the user's write alone.
fn a_losing_offline_write_restores(doc: &str, overtaken: bool) {
    use crate::control_request::Source;
    use crate::invention::declared::{DeclaredSurface, WriteOptions};
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(doc).unwrap();
    let mut out = vec![0.0f32; RATE as usize * 2];
    engine.render_interleaved(&mut out).unwrap();
    engine.set_control("p", "schedule", NEW.into()).unwrap();
    let script = WriteOptions::performed(Source::Script);
    let perform = crate::ControlWriteIntent::Perform;
    let write = || {
        let snapshot = engine.snapshot();
        snapshot.submit_control("p", "schedule", LATER.into(), perform, script)
    };
    let lost = match overtaken {
        true => DeclaredSurface::admitting_all(write),
        false => write(),
    };
    assert!(lost.is_ok(), "{lost:?}");
    let read = engine.get_control("p", "schedule").unwrap();
    assert_eq!(read, ControlValue::String(NEW.into()), "the user's holds");
    let mut rest = vec![0.0f32; RATE as usize * 8];
    engine.render_interleaved(&mut rest).unwrap();
    out.extend(rest);
    assert_eq!(out, render(doc, Some(NEW)));
}

#[test]
fn a_losing_offline_write_restores_what_its_development_recorded() {
    for overtaken in [false, true] {
        a_losing_offline_write_restores(&document(OLD), overtaken);
    }
}

#[test]
fn a_losing_offline_write_restores_what_a_nested_development_recorded() {
    for overtaken in [false, true] {
        a_losing_offline_write_restores(&nested(OLD), overtaken);
    }
}
