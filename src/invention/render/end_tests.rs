//! A block a one-shot ends in can process in pieces: split by a replayed
//! request, or by the graph at a scheduler's entry. The render still finds
//! the `ended` gate where it rose (where the live session's rose), not
//! where the last piece of the block began.

use super::tests::{render_until_end, ONE_SHOT_INVENTION};
use crate::control_request::{RtValue, When};
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::{Invention, InventionBuilder, LoggedRequest, LoggedValue, RenderEngine, RequestLog};

/// The one-shot fixture's end frame, with no replay (see
/// `scan_end_gate_finds_the_exact_end_frame`).
const END: usize = 511;

/// A log of one oscillator-frequency write at `sample`, played in blocks of
/// `block_size`.
fn frequency_write(sample: u64, block_size: usize) -> RequestLog {
    RequestLog {
        sample_rate: SAMPLE_RATE,
        block_size,
        requests: vec![LoggedRequest {
            sample,
            module_id: "osc".into(),
            key: "frequency".into(),
            value: LoggedValue::Number(880.0),
            due: None,
            beat: None,
        }],
        ..RequestLog::default()
    }
}

/// The frame a `--to-end` render of the fixture replaying `log` stops at,
/// for each source a host may name.
fn replayed_end(log: &RequestLog) -> [usize; 2] {
    [None, Some("seq")].map(|source| {
        let mut engine = RenderEngine::new(SAMPLE_RATE);
        engine.load_json(ONE_SHOT_INVENTION).unwrap();
        engine.set_block_size(log.block_size);
        engine.replay(log).unwrap();
        let end = render_until_end(&mut engine, source);
        assert_eq!(engine.replay_refused(), 0);
        end
    })
}

#[test]
fn a_live_sessions_write_inside_the_ending_block_keeps_its_end_frame() {
    // Live: the write applies at sample 480, inside the block [448, 512)
    // the one-shot ends in, and the session's `ended` gate rises in it.
    let invention = Invention::from_json(ONE_SHOT_INVENTION).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    running.live.submit_logged(
        ("osc", "frequency"),
        RtValue::F32(880.0),
        When::AtSample(480),
        None,
    );
    let (played, _) = pump.render_stereo(END / 64);
    assert!(!running.end_reached(None).unwrap(), "not before the block");
    let (last, _) = pump.render_stereo(1);
    assert!(running.end_reached(None).unwrap(), "ended in [448, 512)");
    let log = running.request_log();
    running.stop();
    assert_eq!(log.requests.len(), 1, "{log:?}");
    assert_eq!(log.requests[0].sample, 480);

    // Offline, the replay plays what the session played, and stops where
    // its gate rose: frame 511, not 448 + 31 = 479.
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(ONE_SHOT_INVENTION).unwrap();
    engine.replay(&log).unwrap();
    let mut buffer = [0.0f32; 64 * 2];
    let mut replayed = Vec::new();
    for _ in 0..=END / 64 {
        engine.render_interleaved(&mut buffer).unwrap();
        replayed.extend(buffer.chunks_exact(2).map(|frame| frame[0].to_bits()));
    }
    let bits = played.iter().chain(&last).map(|sample| sample.to_bits());
    assert_eq!(
        replayed,
        bits.collect::<Vec<_>>(),
        "the replay is what played"
    );
    assert_eq!(engine.scan_end_gate(None, 64).unwrap(), Some(END - 448));
    assert_eq!(replayed_end(&log), [END; 2]);
}

#[test]
fn a_replayed_write_anywhere_in_the_ending_block_keeps_its_end_frame() {
    // At the end frame itself: the gate rises at the last piece's start.
    assert_eq!(replayed_end(&frequency_write(511, 64)), [END; 2]);
    // After the end frame, in a larger block: the gate rose in an earlier
    // piece, and the last piece is latched high throughout.
    assert_eq!(replayed_end(&frequency_write(600, 384)), [END; 2]);
    // Several splits, the gate rising in a middle piece.
    let mut log = frequency_write(400, 384);
    let mut later = log.requests[0].clone();
    for sample in [500, 520, 700] {
        later.sample = sample;
        log.requests.push(later.clone());
    }
    assert_eq!(replayed_end(&log), [END; 2]);
}

#[test]
fn a_split_in_an_earlier_block_or_a_short_render_call_keeps_the_end_frame() {
    // A split long before the end: the ending block renders whole.
    assert_eq!(replayed_end(&frequency_write(100, 64)), [END; 2]);

    // Render calls of 48 frames, shorter than the 64-frame block: the call
    // [480, 528) is split at 500, and the gate rose at 511 in its first
    // piece.
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(ONE_SHOT_INVENTION).unwrap();
    engine.replay(&frequency_write(500, 64)).unwrap();
    let mut buffer = [0.0f32; 48 * 2];
    let mut done = 0;
    let end = loop {
        engine.render_interleaved(&mut buffer).unwrap();
        let any = engine.scan_end_gate(None, 48).unwrap();
        assert_eq!(engine.scan_end_gate(Some("seq"), 48).unwrap(), any);
        if let Some(frame) = any {
            break done + frame;
        }
        done += 48;
        assert!(done < 48_000, "end gate never fired");
    };
    assert_eq!(end, END);
}

#[test]
fn a_scheduler_entry_splitting_the_ending_block_keeps_the_end_frame() {
    // The graph splits the block itself at the entry's beat, the end's own
    // (beat edges at 0, 127, 255, 383, 511): no replay needed.
    let mut doc: serde_json::Value = serde_json::from_str(ONE_SHOT_INVENTION).unwrap();
    let entry = serde_json::json!([
        { "at_step": 4, "module": "osc", "control": "frequency", "value": 880.0 }
    ]);
    doc["modules"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "sched", "type": "control_scheduler", "config": { "schedule": entry.to_string() }
        }));
    doc["connections"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "from": "clock", "from_port": "beat", "to": "sched", "to_port": "clock"
        }));
    for source in [None, Some("seq")] {
        let mut engine = RenderEngine::new(SAMPLE_RATE);
        engine.load_json(&doc.to_string()).unwrap();
        assert_eq!(render_until_end(&mut engine, source), END);
    }
}

#[test]
fn an_edit_between_the_render_and_the_scan_keeps_the_end_frame() {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(ONE_SHOT_INVENTION).unwrap();
    engine.replay(&frequency_write(480, 64)).unwrap();
    let mut buffer = [0.0f32; 64 * 2];
    for _ in 0..=END / 64 {
        engine.render_interleaved(&mut buffer).unwrap();
    }
    // Removing the clock moves `seq` to the clock's place in the graph.
    engine.remove_module("clock").unwrap();
    assert_eq!(
        engine.scan_end_gate(Some("seq"), 64).unwrap(),
        Some(END - 448)
    );
    assert_eq!(engine.scan_end_gate(None, 64).unwrap(), Some(END - 448));
}
