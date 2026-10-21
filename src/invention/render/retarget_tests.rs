//! An offline `schedule` write that changes what a scheduler writes
//! recompiles the process order, as the live write does (FUG-388): the
//! render matches the document loaded with the new schedule.

use super::RenderEngine;
use crate::control_request::CURSOR_CAPACITY;
use crate::ControlValue;

const RATE: u32 = 48_000;

/// A clock at 97 bpm stepping `fillers` empty schedulers and then `sched`,
/// playing `schedule`, with the beat gate as the only audio. Past the
/// drain's room, `sched` plays its schedule itself.
fn document(fillers: usize, schedule: &str) -> String {
    let mut modules = vec![
        r#"{"id":"clock","type":"clock","config":{"bpm":97.0}}"#.to_string(),
        r#"{"id":"mix","type":"mixer","config":{"channel_count":1}}"#.to_string(),
    ];
    let mut connections = vec![
        r#"{"from":"clock","from_port":"beat","to":"mix","to_port":"audio.0"}"#.to_string(),
        r#"{"from":"mix","from_port":"audio_left","to":"dac","to_port":"audio_left"}"#.to_string(),
    ];
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

const LEVEL: &str = r#"[{"at_step":6,"module":"mix","control":"level.0","value":0.5}]"#;
const TEMPO: &str = r#"[{"at_step":6,"module":"clock","control":"bpm","value":211.0}]"#;

/// Renders `doc` offline for five seconds, writing `write` to `sched`'s
/// schedule after one.
fn render(doc: &str, write: Option<&str>) -> Vec<f32> {
    let mut engine = RenderEngine::new(RATE);
    engine.load_json(doc).unwrap();
    let mut out = vec![0.0f32; RATE as usize * 2];
    engine.render_interleaved(&mut out).unwrap();
    if let Some(schedule) = write {
        let schedule = ControlValue::String(schedule.into());
        engine.set_control("sched", "schedule", schedule).unwrap();
    }
    let mut rest = vec![0.0f32; RATE as usize * 8];
    engine.render_interleaved(&mut rest).unwrap();
    out.extend(rest);
    out
}

#[test]
fn an_offline_retarget_plays_as_loaded() {
    for fillers in [0, CURSOR_CAPACITY] {
        let loaded = render(&document(fillers, TEMPO), None);
        let written = render(&document(fillers, LEVEL), Some(TEMPO));
        let first = loaded
            .iter()
            .zip(&written)
            .position(|(a, b)| a.to_bits() != b.to_bits());
        assert_eq!(first, None, "{fillers} fillers");
    }
}
