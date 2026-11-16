//! Connected to a voice's `velocity` input, a quieter step is quieter by its
//! stored velocity, with no click or step at a velocity change between
//! notes.
//!
//! The voice is a `sample_instrument`, whose `velocity` input is latched at
//! note-on, playing a generated sample: a 10 ms raised-cosine attack into a
//! steady 0.5. Each note's steady level is measured against the same patch
//! with the velocity output unconnected (full velocity), and the largest
//! sample-to-sample step against that patch's.

use std::path::Path;

use serde_json::json;

use super::rig::*;

const ATTACK: usize = 480;
const LEVEL: f32 = 0.5;

/// Writes the voice's sample into `dir`.
fn write_sample(dir: &Path) -> String {
    let path = dir.join("steady.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for i in 0..2 * RATE as usize {
        let rise = if i < ATTACK {
            0.5 - 0.5 * (std::f32::consts::PI * i as f32 / ATTACK as f32).cos()
        } else {
            1.0
        };
        writer.write_sample(LEVEL * rise).unwrap();
    }
    writer.finalize().unwrap();
    path.to_str().unwrap().to_string()
}

/// Steps with their stored velocities; `None` declares none. Each note is
/// a different key, so no note steals another's voice, and gate 0.9 leaves
/// the previous note's 0.1 s release still sounding as the next one starts.
const STEPS: [Option<(i32, Option<f32>)>; 8] = [
    None,
    Some((0, Some(1.0))),
    Some((2, Some(0.5))),
    Some((4, Some(0.25))),
    Some((5, Some(0.75))),
    Some((7, None)),
    Some((9, Some(0.1))),
    None,
];

fn piece(sample: &str, velocity_connected: bool) -> String {
    let pattern: Vec<_> = STEPS
        .iter()
        .map(|step| match step {
            None => json!({ "note": null }),
            Some((note, None)) => json!({ "note": note }),
            Some((note, Some(velocity))) => json!({ "note": note, "velocity": velocity }),
        })
        .collect();
    let mut connections = vec![
        ["clock", "beat", "seq", "clock"],
        ["seq", "frequency", "keys", "frequency"],
        ["seq", "gate", "keys", "gate"],
        ["keys", "audio_left", "dac", "audio_left"],
        ["clock", "beat", "dac", "audio_right"],
    ];
    if velocity_connected {
        connections.push(["seq", "velocity", "keys", "velocity"]);
    }
    document(
        vec![
            clock(),
            sequencer(json!({
                "root_note": 60, "step_count": 8, "gate_length": 0.9, "pattern": pattern
            })),
            json!({ "id": "keys", "type": "sample_instrument", "config": {
                "voice_count": 8, "release": 0.1,
                "zones": [{ "root_note": 60, "key_range": [0, 127], "asset": { "path": sample } }]
            } }),
            dac(),
        ],
        &connections,
    )
}

/// The mean level over the settled middle of each step, past the previous
/// note's release and the attack.
fn levels(audio: &[f32], edges: &[usize]) -> Vec<f32> {
    edges
        .windows(2)
        .map(|step| {
            let window = &audio[step[0] + 4_000..step[0] + 10_000];
            window.iter().sum::<f32>() / window.len() as f32
        })
        .collect()
}

fn largest_step(audio: &[f32]) -> f32 {
    audio
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn a_quieter_step_is_quieter_by_its_stored_velocity_without_a_click() {
    let dir = tempfile::tempdir().unwrap();
    let sample = write_sample(dir.path());
    let frames = 17 * STEP;
    let (full, beat) = render(&piece(&sample, false), frames);
    let (shaped, beat_again) = render(&piece(&sample, true), frames);
    assert_eq!(beat, beat_again, "both patches keep the same time");
    let edges = rises(&beat);
    assert!(edges.len() >= 17, "two loops of the pattern: {edges:?}");

    let full_levels = levels(&full, &edges[..17]);
    let shaped_levels = levels(&shaped, &edges[..17]);
    for (k, (full_level, shaped_level)) in full_levels.iter().zip(&shaped_levels).enumerate() {
        let Some((_, velocity)) = STEPS[k % 8] else {
            continue;
        };
        let velocity = velocity.unwrap_or(1.0);
        assert!(
            (full_level - LEVEL).abs() < 0.01,
            "step {k} sounds at the sample's level unshaped: {full_level}"
        );
        let ratio = shaped_level / full_level;
        assert!(
            (ratio - velocity).abs() < 0.005,
            "step {k}: level {shaped_level} is {ratio} of full, but its velocity is {velocity}"
        );
    }

    // No click at a change. Unshaped, the largest step is the sample's own
    // attack: 0.5 * pi / 960 per sample at the root key, up to 1.7 times
    // that at the highest key the line plays. Each shaped note is
    // its unshaped self scaled by a velocity of at most 1, so shaping can
    // add to the largest step only where an attack's rise is less offset by
    // an overlapping release's fall (0.5 over 0.1 s, a few percent of the
    // attack's slope). A velocity reaching a note after its onset, or a
    // ringing note's level jumping, would step by up to the note's level.
    let unshaped = largest_step(&full);
    let shaped_step = largest_step(&shaped);
    assert!(
        unshaped < 0.004,
        "the unshaped line is click-free: {unshaped}"
    );
    assert!(
        shaped_step <= unshaped * 1.1,
        "the shaped line steps by {shaped_step}, the unshaped by {unshaped}"
    );
}
