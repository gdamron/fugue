//! A live session with control requests at samples and at beats, exported
//! and replayed offline, matches what the live session sent the DAC, sample
//! for sample: as heard (each channel clamped to ±1, and the mono fold) and
//! before the clamp too.

use crate::control_request::{BeatSpec, BeatTime, ControlIndex, RtValue, When};
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::rpc::ControlWriteIntent;
use crate::{Invention, InventionBuilder, LoggedBeat, RenderEngine, RequestLog};

const DOC: &str = include_str!("../../../../examples/step_sequencer.json");

/// The device's callback size, which is also the graph's block size.
const BLOCK: usize = 64;

/// What a session sent the DAC, per channel.
struct Output {
    left: Vec<f32>,
    right: Vec<f32>,
}

/// A live session of [`DOC`] on a manual backend: a perform and an author
/// write now, writes at samples inside blocks (a tempo change and a reset
/// among them), writes on the clock's beats, and one that applies late.
/// Returns the exported log, through JSON, and what played.
fn live_session() -> (RequestLog, Output) {
    let invention = Invention::from_json(DOC).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    let snapshot = running.snapshot();
    let live = &running.live;
    let clock = {
        let publisher = live.publisher().lock().unwrap();
        let clock = publisher.control_target("clock", ControlIndex(0));
        clock.unwrap().module_idx as u32
    };
    let mut out = Output {
        left: Vec::new(),
        right: Vec::new(),
    };
    let mut play = |blocks| {
        let (left, right) = pump.render_stereo(blocks);
        out.left.extend(left);
        out.right.extend(right);
    };
    let beats = |spec| When::Beat(BeatTime { clock, spec });

    play(100);
    let perform = ControlWriteIntent::Perform;
    let author = ControlWriteIntent::Author;
    snapshot
        .set_control_with_intent("osc", "waveform", "square".into(), perform)
        .unwrap();
    play(50);
    snapshot
        .set_control_with_intent("seq", "gate_length", 0.25.into(), author)
        .unwrap();
    live.submit_logged(
        ("seq", "root_note"),
        RtValue::I32(55),
        When::AtSample(20_011),
        None,
    );
    live.submit_logged(
        ("clock", "bpm"),
        RtValue::F32(97.0),
        When::AtSample(30_007),
        None,
    );
    play(250);
    let after = LoggedBeat::After {
        clock: "clock".into(),
        beats: 2.0,
    };
    live.submit_logged(
        ("osc", "waveform"),
        RtValue::U32(0),
        beats(BeatSpec::After(2.0)),
        Some(after),
    );
    let grid = LoggedBeat::Grid {
        clock: "clock".into(),
        every: 4.0,
        offset: 1.0,
        repeat: false,
    };
    let spec = BeatSpec::Grid {
        every: 4.0,
        offset: 1.0,
        repeat: false,
    };
    live.submit_logged(
        ("seq", "root_note"),
        RtValue::I32(43),
        beats(spec),
        Some(grid),
    );
    live.submit_logged(
        ("clock", "reset"),
        RtValue::Bool(true),
        When::AtSample(80_003),
        None,
    );
    play(1000);
    // Due long ago: applies late, at the next block start.
    live.submit_logged(
        ("seq", "gate_length"),
        RtValue::F32(0.9),
        When::AtSample(100),
        None,
    );
    play(600);

    let log = running.request_log();
    let json = serde_json::to_string(&log).unwrap();
    running.stop();
    (serde_json::from_str(&json).unwrap(), out)
}

/// An offline render of [`DOC`] as loaded, in device-sized calls, replaying
/// `log` when there is one.
fn offline(log: Option<&RequestLog>, frames: usize) -> Output {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(DOC).unwrap();
    if let Some(log) = log {
        engine.replay(log).unwrap();
    }
    let mut out = Output {
        left: Vec::with_capacity(frames),
        right: Vec::with_capacity(frames),
    };
    let mut buffer = [0.0f32; BLOCK * 2];
    for _ in 0..frames / BLOCK {
        engine.render_interleaved(&mut buffer).unwrap();
        for frame in buffer.chunks_exact(2) {
            out.left.push(frame[0]);
            out.right.push(frame[1]);
        }
    }
    assert_eq!(engine.replay_refused(), 0);
    out
}

/// What the DAC sends a stereo device and a mono one, sample by sample:
/// each channel clamped to ±1, and their mono fold.
fn heard(out: &Output) -> Vec<[u32; 3]> {
    let clamp = |sample: f32| sample.clamp(-1.0, 1.0);
    out.left
        .iter()
        .zip(&out.right)
        .map(|(&l, &r)| {
            let (l, r) = (clamp(l), clamp(r));
            [l.to_bits(), r.to_bits(), (0.5 * (l + r)).to_bits()]
        })
        .collect()
}

/// The first sample at which `a` and `b` are heard differently.
fn first_difference(a: &Output, b: &Output) -> Option<usize> {
    heard(a).iter().zip(heard(b)).position(|(a, b)| *a != b)
}

#[test]
fn a_replayed_session_matches_what_played_sample_for_sample() {
    let (log, played) = live_session();
    assert!(log.is_exact(), "{log:?}");
    let samples: Vec<u64> = log.requests.iter().map(|r| r.sample).collect();
    assert_eq!(log.requests.len(), 8, "{samples:?}");
    assert!(
        samples.is_sorted(),
        "apply order is time order: {samples:?}"
    );
    assert!(samples.contains(&20_011) && samples.contains(&30_007));
    assert!(samples.contains(&80_003));
    let beat_timed = log.requests.iter().filter(|r| r.beat.is_some());
    assert_eq!(beat_timed.count(), 2);
    assert_eq!(
        log.requests.iter().filter(|r| r.due == Some(100)).count(),
        1
    );

    let peak = played.left.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    assert!(peak > 0.1, "the session plays: peak {peak}");

    let replayed = offline(Some(&log), played.left.len());
    assert_eq!(first_difference(&played, &replayed), None);
    let bits = |out: &Output| -> Vec<(u32, u32)> {
        let pairs = out.left.iter().zip(&out.right);
        pairs.map(|(l, r)| (l.to_bits(), r.to_bits())).collect()
    };
    assert_eq!(bits(&played), bits(&replayed), "before the clamp too");
}

#[test]
fn a_render_without_the_log_or_one_sample_off_does_not_match() {
    let (log, played) = live_session();
    let frames = played.left.len();
    let first = log.requests[0].sample as usize;
    let without = first_difference(&played, &offline(None, frames));
    assert!(without.is_some_and(|at| at >= first), "{without:?}");

    // Every request one sample late: the render must tell.
    let mut late = log.clone();
    for request in &mut late.requests {
        request.sample += 1;
    }
    assert!(first_difference(&played, &offline(Some(&late), frames)).is_some());
}

#[test]
fn a_log_naming_no_declared_control_here_is_refused() {
    let (mut log, _) = live_session();
    log.requests[0].key = "no_such_control".into();
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(DOC).unwrap();
    let error = engine.replay(&log).unwrap_err().to_string();
    assert!(error.contains("no_such_control"), "{error}");
}
