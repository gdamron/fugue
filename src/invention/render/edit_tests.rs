//! Offline edits keep a live edit's contract with surviving schedulers:
//! after a module they target is replaced, what they schedule reaches the
//! new instance, and removing one they still target is refused with
//! nothing changed.

use super::RenderEngine;
use crate::control_request::CURSOR_CAPACITY;
use crate::ControlValue;

const RATE: u32 = 48_000;

/// A clock at 120 bpm stepping `fillers` empty schedulers and then `sched`,
/// which halves the mixer's level on step 4; the beat gate through the
/// mixer is the only audio. Past the drain's room, `sched` plays its
/// schedule itself.
pub(in crate::invention::render) fn document(fillers: usize) -> String {
    let mut modules = vec![
        r#"{"id":"clock","type":"clock","config":{"bpm":120.0}}"#.to_string(),
        r#"{"id":"mix","type":"mixer","config":{"channel_count":1}}"#.to_string(),
    ];
    let mut connections = vec![
        r#"{"from":"clock","from_port":"beat","to":"mix","to_port":"audio.0"}"#.to_string(),
        r#"{"from":"mix","from_port":"audio_left","to":"dac","to_port":"audio_left"}"#.to_string(),
    ];
    let schedule = r#"[{"at_step":4,"module":"mix","control":"level.0","value":0.5}]"#;
    let schedulers = (0..fillers)
        .map(|i| (format!("f{i}"), "[]"))
        .chain([("sched".into(), schedule)]);
    for (id, schedule) in schedulers {
        modules.push(format!(
            r#"{{"id":"{id}","type":"control_scheduler","config":{{"schedule":{schedule}}}}}"#
        ));
        connections.push(format!(
            r#"{{"from":"clock","from_port":"beat","to":"{id}","to_port":"clock"}}"#
        ));
    }
    modules.push(r#"{"id":"dac","type":"dac","config":{"soft_clip":false}}"#.to_string());
    format!(
        r#"{{"version":"1.0.0","modules":[{}],"connections":[{}]}}"#,
        modules.join(","),
        connections.join(",")
    )
}

fn level(engine: &RenderEngine) -> f32 {
    match engine.get_control("mix", "level.0").unwrap() {
        ControlValue::Number(level) => level,
        other => panic!("{other:?}"),
    }
}

/// Renders `fillers`' document for four seconds, replacing the mixer with
/// a fresh one after half a second when `replace` is set.
fn render(fillers: usize, replace: bool) -> (Vec<f32>, f32) {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(&document(fillers)).unwrap();
    let mut out = vec![0.0f32; RATE as usize];
    engine.render_interleaved(&mut out).unwrap();
    if replace {
        let config = serde_json::json!({ "channel_count": 1 });
        engine.add_module("mix", "mixer", &config).unwrap();
        assert_eq!(engine.list_connections().len(), 3 + fillers);
    }
    let mut rest = vec![0.0f32; RATE as usize * 7];
    engine.render_interleaved(&mut rest).unwrap();
    out.extend(rest);
    (out, level(&engine))
}

#[test]
fn a_schedule_reaches_the_module_that_replaces_its_target_offline() {
    for fillers in [0, CURSOR_CAPACITY] {
        let (kept, _) = render(fillers, false);
        let (replaced, level) = render(fillers, true);
        assert_eq!(level, 0.5, "{fillers} fillers");
        let first = kept
            .iter()
            .zip(&replaced)
            .position(|(a, b)| a.to_bits() != b.to_bits());
        assert_eq!(first, None, "{fillers} fillers");
    }
}

#[test]
fn removing_a_scheduled_target_offline_is_refused_with_nothing_changed() {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(&document(0)).unwrap();
    let controller = engine.controller().unwrap();
    let (modules, connections) = (engine.list_modules(), engine.list_connections());
    let error = engine.remove_module("mix").unwrap_err().to_string();
    assert!(error.contains("sched"), "{error}");
    assert!(controller.remove_module("mix").is_err());
    assert_eq!(engine.list_modules().len(), modules.len());
    assert_eq!(engine.list_connections().len(), connections.len());
    assert!(engine
        .snapshot()
        .document()
        .unwrap()
        .modules
        .iter()
        .any(|m| m.id == "mix"));
    // The schedule still lands on the mixer.
    let mut out = vec![0.0f32; RATE as usize * 8];
    engine.render_interleaved(&mut out).unwrap();
    assert_eq!(level(&engine), 0.5);
    // With a schedule that no longer targets it, the mixer goes.
    let schedule = ControlValue::String("[]".into());
    engine.set_control("sched", "schedule", schedule).unwrap();
    engine.remove_module("mix").unwrap();
    assert_eq!(engine.list_modules().len(), modules.len() - 1);
    assert_eq!(engine.list_connections().len(), connections.len() - 2);
}

/// The sample the schedule's write lands on, rendering a frame at a time.
pub(in crate::invention::render) fn step_sample() -> usize {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(&document(0)).unwrap();
    let mut frame = [0.0f32; 2];
    for sample in 0..RATE as usize * 4 {
        engine.render_interleaved(&mut frame).unwrap();
        if level(&engine) == 0.5 {
            return sample;
        }
    }
    panic!("the schedule never landed");
}

#[test]
fn a_scheduler_removed_offline_on_its_step_still_plays_it_as_live() {
    let at = step_sample();
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(&document(0)).unwrap();
    let mut out = vec![0.0f32; at * 2];
    engine.render_interleaved(&mut out).unwrap();
    assert_eq!(level(&engine), 1.0);
    // Live, a table the edit removes plays its step on the graph the edit
    // replaces, before the install at the same sample.
    engine.remove_module("sched").unwrap();
    engine.render_interleaved(&mut [0.0f32; 2]).unwrap();
    assert_eq!(level(&engine), 0.5);
}
