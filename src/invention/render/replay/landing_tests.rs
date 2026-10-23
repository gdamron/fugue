//! A replayed timed edit plays what live played around it: the step of a
//! scheduler it removes on that step, the values a batch set on a module
//! it built, and the clock latch of an edit taken on its beat.

use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::invention::render::edit_tests::{document, step_sample};
use crate::rpc::{EditWhen, StructuralEdit};
use crate::{ControlValue, Invention, InventionBuilder, LoggedSetting, RenderEngine, RequestLog};

const BLOCK: usize = 64;

/// A live session of the scheduler document, `batch` queued `when` after
/// `blocks.0`, for `blocks.1` more: the exported log, through JSON, and
/// what played.
fn session(
    batch: &[StructuralEdit],
    when: &EditWhen,
    blocks: (usize, usize),
) -> (RequestLog, Vec<f32>) {
    let invention = Invention::from_json(&document(0)).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (mut running, pump) = start_manual(runtime);
    let mut out = Vec::new();
    let mut play = |blocks| {
        let (left, right) = pump.render_stereo(blocks);
        for (l, r) in left.into_iter().zip(right) {
            out.extend([l, r]);
        }
    };
    play(blocks.0);
    running.apply_edits_at(batch, when, None).unwrap();
    play(blocks.1);
    let json = serde_json::to_string(&running.request_log()).unwrap();
    running.stop();
    (serde_json::from_str(&json).unwrap(), out)
}

/// Replays `log` over the document as loaded, asserting it renders what
/// `played`, bit for bit; returns the engine.
fn replayed(log: &RequestLog, played: &[f32]) -> RenderEngine {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(&document(0)).unwrap();
    engine.replay(log).unwrap();
    let mut replayed = vec![0.0f32; played.len()];
    for chunk in replayed.chunks_mut(BLOCK * 2) {
        engine.render_interleaved(chunk).unwrap();
    }
    assert_eq!(engine.replay_refused(), 0);
    let same = |(a, b): (&f32, &f32)| a.to_bits() == b.to_bits();
    let first = played.iter().zip(&replayed).position(|pair| !same(pair));
    assert_eq!(first, None);
    engine
}

fn add_tone() -> StructuralEdit {
    StructuralEdit::AddModule {
        id: "tone".into(),
        module_type: "oscillator".into(),
        config: serde_json::json!({}),
    }
}

fn set(module_id: &str, key: &str, value: f32) -> StructuralEdit {
    StructuralEdit::SetControl {
        module_id: module_id.into(),
        key: key.into(),
        value: ControlValue::Number(value),
    }
}

#[test]
fn a_replayed_edit_plays_the_step_it_removes_and_sets_what_it_builds() {
    // At the scheduler's step, one batch removes it and adds an oscillator
    // set to 330 Hz on the right.
    let at = step_sample() as u64;
    let batch = [
        StructuralEdit::RemoveModule { id: "sched".into() },
        add_tone(),
        set("tone", "frequency", 330.0),
        StructuralEdit::Connect {
            from: "tone".into(),
            from_port: "audio".into(),
            to: "dac".into(),
            to_port: "audio_right".into(),
        },
    ];
    let when = EditWhen::AtSample { sample: at };
    let (log, played) = session(&batch, &when, (0, at as usize / BLOCK + 200));
    assert!(log.is_exact(), "{log:#?}");
    assert_eq!(log.timed_edits[0].landed(), Some(at));
    let frequency = LoggedSetting {
        key: "frequency".into(),
        value: ControlValue::Number(330.0),
    };
    assert_eq!(log.timed_edits[0].built[0].writes, [frequency]);

    let engine = replayed(&log, &played);
    // The removed scheduler's step was heard: the mixer's level halved.
    assert_eq!(
        engine.get_control("mix", "level.0").unwrap(),
        ControlValue::Number(0.5)
    );
}

#[test]
fn a_replayed_edit_on_a_beat_latches_its_clock_before_its_tempo_write() {
    // On the next bar, a batch adds a module and slows the clock it counts
    // on: live, the clock latches on the beat the edit is taken, then the
    // tempo applies with the batch.
    let batch = [add_tone(), set("clock", "bpm", 90.0)];
    let bar = EditWhen::NextBeat {
        every: 4.0,
        offset: 0.0,
        clock: None,
    };
    let (log, played) = session(&batch, &bar, (500, 4_000));
    assert!(log.is_exact(), "{log:#?}");
    assert!(log.timed_edits[0].landed().is_some());
    replayed(&log, &played);
}
