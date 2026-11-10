//! Criterion: offline output equals the live device's for the same input,
//! with the same clamp and the same handling of non-finite samples (silence,
//! counted).

use super::support::*;

/// Offline equals the device's clamp of what the graph hands a live backend.
fn assert_matches_device(json: &str, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let live = Live::start(json);
    let graph_output = live.capture(frames);
    live.running.stop();
    let offline = offline(json, frames, 256);
    assert_eq!(offline.len(), graph_output.len());
    for (i, (got, raw)) in offline.iter().zip(&graph_output).enumerate() {
        assert_eq!(
            got.to_bits(),
            device(*raw).to_bits(),
            "sample {i}: graph produced {raw}, offline wrote {got}"
        );
    }
    (graph_output, offline)
}

#[test]
fn over_level_tones_match_the_device_sample_for_sample() {
    for (waveform, db) in [
        ("sine", 12.0),
        ("sawtooth", 18.0),
        ("square", 6.0),
        ("triangle", 9.0),
    ] {
        let (raw, _) = assert_matches_device(&tone(waveform, 311.0, db), 9_600);
        assert!(
            raw.iter().any(|x| x.abs() > 1.5),
            "{waveform} drives the graph over level"
        );
    }
}

#[test]
fn non_finite_samples_are_silence_like_the_device() {
    let json = overflowing_chain();
    let (raw, out) = assert_matches_device(&json, 9_600);
    assert!(
        out.iter().all(|x| x.is_finite()),
        "nothing non-finite is written"
    );
    assert!(
        out.iter().all(|x| x.abs() <= 1.0),
        "the huge but finite samples are clamped"
    );
    // The dac turned the overflow into zeros before the clamp ever saw it,
    // so those samples are silence, not full scale.
    let silent = out.iter().filter(|x| **x == 0.0).count();
    assert!(silent >= out.len() * 80 / 100, "{silent} of {}", out.len());
    assert!(
        raw.iter().all(|x| x.is_finite()),
        "the graph hands over finite samples"
    );
    assert!(
        out.iter().any(|x| x.abs() > 0.5),
        "the finite samples near the crossings are loud"
    );
}
