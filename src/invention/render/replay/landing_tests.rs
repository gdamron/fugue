//! A replayed timed edit plays what live played around it: the step of a
//! scheduler it removes on that step, and the values a batch set on a
//! module it built.

use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::invention::render::edit_tests::{document, step_sample};
use crate::rpc::{EditWhen, StructuralEdit};
use crate::{ControlValue, Invention, InventionBuilder, LoggedSetting, RenderEngine, RequestLog};

const BLOCK: usize = 64;

/// A live session of the scheduler document: at the scheduler's step, one
/// batch removes it and adds an oscillator set to 330 Hz on the right.
/// Returns the exported log, through JSON, and what played.
fn live_session(at: u64) -> (RequestLog, Vec<f32>) {
    let invention = Invention::from_json(&document(0)).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (mut running, pump) = start_manual(runtime);
    let batch = [
        StructuralEdit::RemoveModule { id: "sched".into() },
        StructuralEdit::AddModule {
            id: "tone".into(),
            module_type: "oscillator".into(),
            config: serde_json::json!({}),
        },
        StructuralEdit::SetControl {
            module_id: "tone".into(),
            key: "frequency".into(),
            value: ControlValue::Number(330.0),
        },
        StructuralEdit::Connect {
            from: "tone".into(),
            from_port: "audio".into(),
            to: "dac".into(),
            to_port: "audio_right".into(),
        },
    ];
    let when = EditWhen::AtSample { sample: at };
    running.apply_edits_at(&batch, &when, None).unwrap();
    let mut out = Vec::new();
    let (left, right) = pump.render_stereo(at as usize / BLOCK + 200);
    for (l, r) in left.into_iter().zip(right) {
        out.extend([l, r]);
    }
    let json = serde_json::to_string(&running.request_log()).unwrap();
    running.stop();
    (serde_json::from_str(&json).unwrap(), out)
}

#[test]
fn a_replayed_edit_plays_the_step_it_removes_and_sets_what_it_builds() {
    let at = step_sample() as u64;
    let (log, played) = live_session(at);
    assert!(log.is_exact(), "{log:#?}");
    assert_eq!(log.timed_edits[0].landed(), Some(at));
    let frequency = LoggedSetting {
        key: "frequency".into(),
        value: ControlValue::Number(330.0),
    };
    assert_eq!(log.timed_edits[0].built[0].writes, [frequency]);

    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(&document(0)).unwrap();
    engine.replay(&log).unwrap();
    let mut replayed = vec![0.0f32; played.len()];
    for chunk in replayed.chunks_mut(BLOCK * 2) {
        engine.render_interleaved(chunk).unwrap();
    }
    assert_eq!(engine.replay_refused(), 0);
    let same = |(a, b): (&f32, &f32)| a.to_bits() == b.to_bits();
    let first = played.iter().zip(&replayed).position(|pair| !same(pair));
    assert_eq!(first, None, "the edit landed on {at}");
    // The removed scheduler's step was heard: the mixer's level halved.
    assert_eq!(
        engine.get_control("mix", "level.0").unwrap(),
        ControlValue::Number(0.5)
    );
}
