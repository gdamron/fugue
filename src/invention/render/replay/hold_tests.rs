//! A replay re-imposes the holds a live session's writes kept against a
//! scheduler (A4): a write mid-ramp holds the control until the next
//! entry, and a late write holds from the sample it was due at, so an
//! entry at the very sample it applied takes the control, live and in the
//! replay alike.

use crate::control_request::{RtValue, When};
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::{Invention, InventionBuilder, RenderEngine, RequestLog};

/// A 1 Hz square through `dyn`, whose level a scheduler sets, a step per
/// beat of a clock at 117.8 bpm: step 4's entry fires on the first frame of
/// a block.
const DOC: &str = r#"{
    "version": "1.0.0",
    "modules": [
        { "id": "clock", "type": "clock", "config": { "bpm": 117.8 } },
        { "id": "osc", "type": "oscillator", "config": { "waveform": "square", "frequency": 1 } },
        { "id": "dyn", "type": "vca", "config": { "level": 0.1 } },
        { "id": "sched", "type": "control_scheduler", "config": { "schedule": [
            { "at_step": 0, "module": "dyn", "control": "level", "value": 0.1 },
            { "at_step": 1, "module": "dyn", "control": "level", "value": 0.9, "ramp_steps": 6 },
            { "at_step": 4, "module": "dyn", "control": "level", "value": 0.2, "ramp_steps": 2 },
            { "at_step": 6, "module": "dyn", "control": "level", "value": 0.5 }
        ] } },
        { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
    ],
    "connections": [
        { "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock" },
        { "from": "osc", "from_port": "audio", "to": "dyn", "to_port": "audio" },
        { "from": "dyn", "from_port": "audio", "to": "dac", "to_port": "audio_left" },
        { "from": "dyn", "from_port": "audio", "to": "dac", "to_port": "audio_right" }
    ]
}"#;

const BLOCK: usize = 64;
/// The step 4 entry's sample, a block start.
const STEP_4: u64 = 97_792;

/// A live session: a user's write mid-ramp, then, at the block where step
/// 4's entry fires, a write due long ago. Returns the log and the left
/// channel played.
fn live_session() -> (RequestLog, Vec<f32>) {
    let invention = Invention::from_json(DOC).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    let mut played = pump.render(1000);
    running.set_control("dyn", "level", 0.3.into()).unwrap();
    played.extend(pump.render(STEP_4 as usize / BLOCK - 1000));
    running.live.submit_logged(
        ("dyn", "level"),
        RtValue::F32(0.7),
        When::AtSample(100),
        None,
    );
    played.extend(pump.render(1000));
    let log = running.request_log();
    running.stop();
    (log, played)
}

/// An offline render of [`DOC`] replaying `log`: its left channel.
fn replayed(log: &RequestLog, frames: usize) -> Vec<f32> {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(DOC).unwrap();
    engine.replay(log).unwrap();
    let mut left = Vec::with_capacity(frames);
    let mut buffer = [0.0f32; BLOCK * 2];
    for _ in 0..frames / BLOCK {
        engine.render_interleaved(&mut buffer).unwrap();
        left.extend(buffer.chunks_exact(2).map(|frame| frame[0]));
    }
    assert_eq!(engine.replay_refused(), 0);
    left
}

fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|sample| sample.to_bits()).collect()
}

#[test]
fn a_replay_holds_off_the_scheduler_as_the_session_did() {
    let (log, played) = live_session();
    assert!(log.is_exact(), "{log:?}");
    let applied: Vec<_> = log.requests.iter().map(|r| (r.sample, r.due)).collect();
    assert_eq!(applied, [(64_000, None), (STEP_4, Some(100))]);
    // Held at the user's 0.3 until step 4, then ramping from the late 0.7
    // towards 0.2: the entry at its very sample took the control.
    let level = |at: usize| played[at].abs();
    assert_eq!(level(95_000), 0.3, "held through the ramp");
    assert!((0.2..0.7).contains(&level(130_000)), "{}", level(130_000));

    assert_eq!(bits(&replayed(&log, played.len())), bits(&played));
}

/// A replay that held the late write from where it applied, not from where
/// it was due, would hold step 4's entry off: the negative control.
#[test]
fn a_replay_holding_a_late_write_from_where_it_applied_does_not_match() {
    let (mut log, played) = live_session();
    log.requests[1].due = None;
    let replayed = replayed(&log, played.len());
    assert_eq!(replayed[130_000].abs(), 0.7, "held off");
    assert_ne!(bits(&replayed), bits(&played));

    // A logged due past the sample it applied at holds from that sample;
    // unclamped, past step 6's entry, it would hold that one off too.
    log.requests[1].due = Some(STEP_4 + 60_000);
    assert_eq!(bits(&self::replayed(&log, played.len())), bits(&replayed));
}
