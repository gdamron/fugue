//! A step sequencer's velocities stay unheard while its `velocity` output is
//! unconnected: the line it plays, and its gate, are bit for bit the same
//! with or without stored velocities.

use serde_json::json;

use super::rig::*;

/// A bass voice the sequencer plays without its velocity output.
fn voice(pattern: serde_json::Value) -> String {
    document(
        vec![
            clock(),
            sequencer(json!({
                "root_note": 48, "step_count": 8, "gate_length": 0.6, "pattern": pattern
            })),
            json!({ "id": "osc", "type": "oscillator", "config": { "waveform": "sawtooth" } }),
            json!({ "id": "adsr", "type": "adsr",
                    "config": { "attack": 0.005, "decay": 0.1, "sustain": 0.6, "release": 0.1 } }),
            json!({ "id": "vca", "type": "vca" }),
            dac(),
        ],
        &[
            ["clock", "beat", "seq", "clock"],
            ["seq", "frequency", "osc", "frequency"],
            ["seq", "gate", "adsr", "gate"],
            ["osc", "audio", "vca", "audio"],
            ["adsr", "envelope", "vca", "level"],
            ["vca", "audio", "dac", "audio_left"],
            ["seq", "gate", "dac", "audio_right"],
        ],
    )
}

#[test]
fn stored_velocities_change_nothing_while_the_output_is_unconnected() {
    // The same line with and without stored velocities, graces included:
    // with the output unconnected, the velocities are not heard, and the
    // frequency, gate and step it plays are the same.
    let with = json!([
        { "note": 0, "velocity": 0.2 },
        { "note": 7, "velocity": 0.9, "grace": [5] },
        { "note": null },
        { "note": 3, "velocity": 0.5 },
        { "held": true },
        { "note": 12, "velocity": 0.0 },
        { "note": 10, "velocity": 0.7, "grace": [8, 9] },
        { "note": 0 }
    ]);
    let without = json!([
        { "note": 0 },
        { "note": 7, "grace": [5] },
        { "note": null },
        { "note": 3 },
        { "held": true },
        { "note": 12 },
        { "note": 10, "grace": [8, 9] },
        { "note": 0 }
    ]);
    let frames = 17 * STEP;
    let heard = render(&voice(with), frames);
    let before = render(&voice(without), frames);
    assert!(
        before.0.iter().any(|x| x.abs() > 0.1),
        "the line is audible"
    );
    for (i, (got, want)) in heard.0.iter().zip(&before.0).enumerate() {
        assert_eq!(got.to_bits(), want.to_bits(), "audio sample {i}");
    }
    for (i, (got, want)) in heard.1.iter().zip(&before.1).enumerate() {
        assert_eq!(got.to_bits(), want.to_bits(), "gate sample {i}");
    }
}
