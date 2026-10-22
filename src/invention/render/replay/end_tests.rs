//! A replayed request inside the block a one-shot ends in splits that
//! block; the render still finds the `ended` gate where the live session's
//! rose, not where the last piece of the block began.

use crate::control_request::{RtValue, When};
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::invention::render::tests::{render_until_end, ONE_SHOT_INVENTION};
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
