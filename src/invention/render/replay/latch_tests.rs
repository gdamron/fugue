//! A watch refused on its beat still latched its clock there live, so a
//! tempo written to that clock later in the same sample took effect from
//! the next: the log records the latch, and a replay makes it, so the
//! render still matches what played sample for sample.

use crate::control_request::{BeatSpec, BeatTime, ControlIndex, RtValue, When};
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::invention::RunningInvention;
use crate::rpc::{ControlWriteIntent, EditWhen, StructuralEdit};
use crate::{Invention, InventionBuilder, LoggedBeat, LoggedEditOutcome, RenderEngine, RequestLog};

const BLOCK: usize = 64;

/// The step sequencer's line, with a second clock beating with the first
/// (heard through nothing) and a spare oscillator.
fn document() -> String {
    let mut doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../../examples/step_sequencer.json")).unwrap();
    let modules = doc["modules"].as_array_mut().unwrap();
    modules
        .push(serde_json::json!({ "id": "clock2", "type": "clock", "config": { "bpm": 120.0 } }));
    modules.push(serde_json::json!({ "id": "spare", "type": "oscillator" }));
    doc.to_string()
}

/// On the next bar of `clock`, as a request is timed and logged.
fn bar(running: &RunningInvention, clock: &str) -> (When, LoggedBeat) {
    let publisher = running.live.publisher().lock().unwrap();
    let target = publisher.control_target(clock, ControlIndex(0)).unwrap();
    let spec = BeatSpec::Grid {
        every: 4.0,
        offset: 0.0,
        repeat: false,
    };
    let when = When::Beat(BeatTime {
        clock: target.module_idx as u32,
        spec,
        ramp: None,
    });
    let logged = LoggedBeat::Grid {
        clock: clock.into(),
        every: 4.0,
        offset: 0.0,
        repeat: false,
    };
    (when, logged)
}

/// A live session of [`document`]: after a while, `refuse` queues
/// something on the next bar of `clock` that is refused there, then the
/// main clock's tempo is written on the same bar of `clock2`, which latches
/// only `clock2`. Returns the log, through JSON, and what played.
fn session(refuse: impl FnOnce(&mut RunningInvention)) -> (RequestLog, Vec<f32>) {
    let invention = Invention::from_json(&document()).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (mut running, pump) = start_manual(runtime);
    let mut out = Vec::new();
    let mut play = |blocks| {
        let (left, right) = pump.render_stereo(blocks);
        for (l, r) in left.into_iter().zip(right) {
            out.extend([l, r]);
        }
    };
    play(300);
    refuse(&mut running);
    let (when, beat) = bar(&running, "clock2");
    let tempo = ("clock", "bpm");
    running
        .live
        .submit_logged(tempo, RtValue::F32(90.0), when, Some(beat));
    play(2_000);
    let json = serde_json::to_string(&running.request_log()).unwrap();
    running.stop();
    (serde_json::from_str(&json).unwrap(), out)
}

/// Replays `log` over [`document`] as loaded, for as long as `played`:
/// the first frame it renders differently, if any.
fn replay(log: &RequestLog, played: &[f32]) -> Option<usize> {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(&document()).unwrap();
    engine.replay(log).unwrap();
    let mut replayed = vec![0.0f32; played.len()];
    for chunk in replayed.chunks_mut(BLOCK * 2) {
        engine.render_interleaved(chunk).unwrap();
    }
    assert_eq!(engine.replay_refused(), 0);
    let same = |(a, b): (&f32, &f32)| a.to_bits() == b.to_bits();
    let first = played.iter().zip(&replayed).position(|pair| !same(pair));
    first.map(|at| at / 2)
}

/// The tempo write's sample, which the one latch logged shares.
fn latched_with_the_tempo(log: &RequestLog) -> u64 {
    assert!(log.is_exact(), "{log:#?}");
    let tempo = log.requests.iter().find(|r| r.key == "bpm").unwrap();
    let [latch] = &log.latches[..] else {
        panic!("{:?}", log.latches);
    };
    assert_eq!(
        (latch.clock.as_str(), latch.sample),
        ("clock", tempo.sample)
    );
    tempo.sample
}

#[test]
fn a_beat_request_refused_on_its_beat_latches_its_clock_in_a_replay() {
    let (log, played) = session(|running| {
        let (when, beat) = bar(running, "clock");
        // A choice past the oscillator's options: refused as it applies.
        let waveform = ("osc", "waveform");
        running
            .live
            .submit_logged(waveform, RtValue::U32(99), when, Some(beat));
    });
    let at = latched_with_the_tempo(&log);
    assert_eq!(replay(&log, &played), None, "latched on {at}");
}

#[test]
fn a_beat_edit_refused_on_its_beat_latches_its_clock_in_a_replay() {
    let (log, played) = session(|running| {
        let batch = [StructuralEdit::RemoveModule { id: "spare".into() }];
        let bar = EditWhen::NextBeat {
            every: 4.0,
            offset: 0.0,
            clock: Some("clock".into()),
        };
        running.apply_edits_at(&batch, &bar, None).unwrap();
        // An authored write to the module it removes cancels it: refused
        // once due, on its beat.
        let author = ControlWriteIntent::Author;
        running
            .snapshot()
            .set_control_with_intent("spare", "frequency", 330.0.into(), author)
            .unwrap();
    });
    let refused = LoggedEditOutcome::Refused {
        reason: "target_gone".into(),
    };
    assert_eq!(log.timed_edits[0].outcome, refused);
    let at = latched_with_the_tempo(&log);
    assert_eq!(replay(&log, &played), None, "latched on {at}");
}

#[test]
fn a_log_from_before_latches_were_recorded_replays_as_it_did() {
    let (log, played) = session(|running| {
        let (when, beat) = bar(running, "clock");
        let waveform = ("osc", "waveform");
        running
            .live
            .submit_logged(waveform, RtValue::U32(99), when, Some(beat));
    });
    let at = latched_with_the_tempo(&log);
    // Such a log has no `latches`; one with none writes none.
    let mut old = serde_json::to_value(&log).unwrap();
    old.as_object_mut().unwrap().remove("latches");
    let old: RequestLog = serde_json::from_value(old).unwrap();
    assert!(old.latches.is_empty());
    let written = serde_json::to_value(&old).unwrap();
    assert!(written.get("latches").is_none(), "{written}");
    // It replays without the latch: the tempo then takes effect a sample
    // early, and the render parts from what played after it.
    let parted = replay(&old, &played).expect("the tempo is heard");
    assert!(parted as u64 >= at, "parted at {parted}, latched on {at}");
}

#[test]
fn a_repeating_beat_request_refused_on_its_beat_leaves_the_log_inexact() {
    let (log, _) = session(|running| {
        let (When::Beat(mut time), LoggedBeat::Grid { clock, .. }) = bar(running, "clock") else {
            unreachable!();
        };
        time.spec = BeatSpec::Grid {
            every: 4.0,
            offset: 0.0,
            repeat: true,
        };
        let beat = LoggedBeat::Grid {
            clock,
            every: 4.0,
            offset: 0.0,
            repeat: true,
        };
        let waveform = ("osc", "waveform");
        running
            .live
            .submit_logged(waveform, RtValue::U32(99), When::Beat(time), Some(beat));
    });
    // It may have lost more beats than it reports: no latch says where.
    assert!(log.latches.is_empty());
    assert_eq!(log.unlogged, 1);
    assert!(!log.is_exact());
}

#[test]
fn a_repeating_beat_request_still_waiting_leaves_the_log_inexact() {
    let (log, _) = session(|running| {
        let (When::Beat(mut time), LoggedBeat::Grid { clock, .. }) = bar(running, "clock") else {
            unreachable!();
        };
        // Far off: still waiting when the log is exported.
        time.spec = BeatSpec::Grid {
            every: 4096.0,
            offset: 0.0,
            repeat: true,
        };
        let beat = LoggedBeat::Grid {
            clock,
            every: 4096.0,
            offset: 0.0,
            repeat: true,
        };
        let waveform = ("osc", "waveform");
        running
            .live
            .submit_logged(waveform, RtValue::U32(1), When::Beat(time), Some(beat));
    });
    // It may have lost beats it latched on, and reports them only as it
    // ends.
    assert_eq!(log.unlogged, 1);
    assert!(!log.is_exact());
}
