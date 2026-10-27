//! A replay finishes a late note off before the late release due after it
//! applies, as the live session did: the note fades over the release in
//! force at its own sample, live and in the replay alike.

use crate::control_request::{RtValue, When};
use crate::invention::manual_backend::{start_manual, SAMPLE_RATE};
use crate::test_support::sampler::wav;
use crate::{Invention, InventionBuilder, RenderEngine, RequestLog};

const BLOCK: usize = 64;

/// A sample instrument with a 1 ms release, its zone a second of half
/// scale at `asset`, into the DAC.
fn doc(asset: &str) -> String {
    serde_json::json!({
        "version": "1.0.0",
        "modules": [
            { "id": "inst", "type": "sample_instrument", "config": {
                "release": 0.001,
                "zones": [{ "root_note": 60, "key_range": [0, 127], "asset": asset }]
            } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "inst", "from_port": "audio_left", "to": "dac", "to_port": "audio_left" }
        ]
    })
    .to_string()
}

/// A live session: note 60 held, then, both late, its note off and a
/// one-second release due two samples after it. Returns the log and the
/// left channel played.
fn live_session(doc: &str) -> (RequestLog, Vec<f32>) {
    let invention = Invention::from_json(doc).unwrap();
    let (runtime, _) = InventionBuilder::new(SAMPLE_RATE).build(invention).unwrap();
    let (running, pump) = start_manual(runtime);
    running.set_control("inst", "note_on", 60.0.into()).unwrap();
    let mut played = pump.render(10);
    let now = played.len() as u64;
    let late = |key, value, due| {
        let when = When::AtSample(due);
        running
            .live
            .submit_logged(("inst", key), RtValue::F32(value), when, None);
    };
    late("note_off", 60.0, now - 4);
    late("release", 1.0, now - 2);
    played.extend(pump.render(10));
    let log = running.request_log();
    running.stop();
    (log, played)
}

fn replayed(doc: &str, log: &RequestLog, frames: usize) -> Vec<f32> {
    let mut engine = RenderEngine::new(SAMPLE_RATE);
    engine.load_json(doc).unwrap();
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
fn a_replay_finishes_a_late_note_off_before_a_later_late_release() {
    let dir = tempfile::tempdir().unwrap();
    let doc = doc(&wav(dir.path(), "a.wav", 44_100));
    let (log, played) = live_session(&doc);
    assert!(
        played[10 * BLOCK - 1] > 0.4,
        "held: {}",
        played[10 * BLOCK - 1]
    );
    assert_eq!(played[played.len() - 1], 0.0, "the old release ended it");
    assert_eq!(bits(&replayed(&doc, &log, played.len())), bits(&played));
}
