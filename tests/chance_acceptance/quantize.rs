//! Quantization: every quantized value lies in the scale and key set by the
//! `key`, `scale`, `low_note`, `range` and `steps` controls.

use crate::rig::*;
use std::collections::HashSet;

pub fn midi_of(frequency: f32) -> f64 {
    69.0 + 12.0 * (frequency as f64 / 440.0).log2()
}

pub fn frequency_of(midi: f64) -> f64 {
    440.0 * 2f64.powf((midi - 69.0) / 12.0)
}

pub fn in_scale(midi: i64, tonic: i32, intervals: &[i32]) -> bool {
    let degree = (midi - tonic as i64).rem_euclid(12) as i32;
    intervals.contains(&degree)
}

// ---------------------------------------------------------------------------
// Criterion 5: 100% of quantized values are in the scale and key set by the
// key and scale controls
// ---------------------------------------------------------------------------

/// Asserts every frequency a quantized channel emits is an exact
/// equal-tempered note of `key` and `scale`.
fn assert_all_quantized(steps: &[Step], tonic: i32, intervals: &[i32], context: &str) {
    let mut seen_pitch_classes: HashSet<i64> = HashSet::new();
    let mut channel_live = [false; 3];
    for (i, s) in steps.iter().enumerate() {
        // Channels 1 and 2 hold until their gate first rises; judge them after.
        channel_live[0] |= s.fired(0);
        channel_live[1] |= s.fired(1);
        channel_live[2] = true;
        for ch in 0..3 {
            if !channel_live[ch] {
                continue;
            }
            let f = s.freq[ch];
            assert!(
                f.is_finite() && f > 0.0,
                "{context}: step {i} channel {} gave {f}",
                ch + 1
            );
            let midi = midi_of(f);
            let note = midi.round();
            assert!(
                (midi - note).abs() < 1e-3,
                "{context}: step {i} channel {} at {f} Hz is {midi} semitones, not an exact note",
                ch + 1
            );
            assert!(
                in_scale(note as i64, tonic, intervals),
                "{context}: step {i} channel {} gave MIDI note {note}, outside the scale",
                ch + 1
            );
            seen_pitch_classes.insert((note as i64 - tonic as i64).rem_euclid(12));
        }
    }
    assert!(
        seen_pitch_classes.len() >= 2.min(intervals.len()),
        "{context}: only {} pitch classes ever sounded",
        seen_pitch_classes.len()
    );
}

#[test]
fn every_quantized_value_is_in_every_scale_and_key() {
    for (scale, intervals) in SCALES {
        for (key, tonic) in KEYS {
            let mut r = rig(serde_json::json!({
                "key": key, "scale": scale, "steps": 0.5, "spread": 1.0,
                "mode": "independent", "bias": 0.7, "seed": 99
            }));
            let s = r.run(400);
            assert_all_quantized(&s, tonic, intervals, &format!("{key} {scale} steps 0.5"));
        }
    }
}

#[test]
fn quantizing_toward_the_strongest_degrees_stays_in_the_scale() {
    // `steps` at 0.5 and above quantizes; nearing 1 it narrows toward the
    // scale's strongest degrees, which are still scale notes.
    let combos = [
        ("C", "major"),
        ("D", "minor"),
        ("F#", "dorian"),
        ("A", "blues"),
        ("Eb", "major_pentatonic"),
        ("G", "whole_tone"),
        ("B", "harmonic_minor"),
        ("A#", "lydian"),
    ];
    for steps in [0.5f64, 0.6, 0.75, 0.9, 1.0] {
        for (key, scale) in combos {
            let tonic = KEYS
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, t)| *t)
                .unwrap_or(3); // Eb
            let intervals = SCALES.iter().find(|(s, _)| *s == scale).unwrap().1;
            let mut r = rig(serde_json::json!({
                "key": key, "scale": scale, "steps": steps, "spread": 1.0,
                "mode": "independent", "bias": 0.7, "seed": 17
            }));
            let s = r.run(1_500);
            assert_all_quantized(
                &s,
                tonic,
                intervals,
                &format!("{key} {scale} steps {steps}"),
            );
        }
    }
}

#[test]
fn quantization_follows_low_note_and_range() {
    for (low_note, range) in [(36, 12), (60, 7), (24, 48), (48, 24)] {
        let mut r = rig(serde_json::json!({
            "key": "D", "scale": "minor", "low_note": low_note, "range": range,
            "spread": 1.0, "steps": 0.5, "seed": 21
        }));
        let s = r.run(1_500);
        assert_all_quantized(
            &s,
            2,
            &[0, 2, 3, 5, 7, 8, 10],
            &format!("low {low_note} range {range}"),
        );
        let (lo, hi) = (low_note as f64 - 2.0, (low_note + range) as f64 + 2.0);
        for (i, step) in s.iter().enumerate() {
            let midi = midi_of(step.freq[2]);
            assert!(
                midi >= lo && midi <= hi,
                "low {low_note} range {range}: step {i} sounded MIDI {midi}"
            );
        }
        let top = s
            .iter()
            .map(|x| midi_of(x.freq[2]))
            .fold(f64::MIN, f64::max);
        let bottom = s
            .iter()
            .map(|x| midi_of(x.freq[2]))
            .fold(f64::MAX, f64::min);
        assert!(
            top - bottom >= (range as f64) * 0.6,
            "low {low_note} range {range}: spread 1 should cover most of the range, got {bottom}..{top}"
        );
    }
}

#[test]
fn quantized_pitch_is_the_nearest_scale_note_to_the_mapped_value() {
    // Interface: pitch = low_note + value * range; quantized it is the
    // nearest MIDI note in the scale and key.
    let (low_note, range) = (48.0f64, 24.0f64);
    let tonic = 9; // A
    let intervals: &[i32] = &[0, 2, 3, 5, 7, 8, 10]; // minor
    let mut r = rig(serde_json::json!({
        "key": "A", "scale": "minor", "steps": 0.5, "spread": 1.0, "seed": 64
    }));
    let mut compared = 0;
    for (i, s) in r.run(1_500).iter().enumerate() {
        let raw = low_note + s.value[2] as f64 * range;
        let mut scored: Vec<(f64, i64)> = ((raw.floor() as i64 - 12)..=(raw.ceil() as i64 + 12))
            .filter(|m| in_scale(*m, tonic, intervals))
            .map(|m| ((m as f64 - raw).abs(), m))
            .collect();
        scored.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if scored[1].0 - scored[0].0 < 0.02 {
            continue; // a tie between two notes is allowed to go either way
        }
        compared += 1;
        assert_eq!(
            midi_of(s.freq[2]).round() as i64,
            scored[0].1,
            "step {i}: value {} maps to {raw}, nearest scale note is {}",
            s.value[2],
            scored[0].1
        );
    }
    assert!(compared > 1_000);
}

#[test]
fn unquantized_stepped_band_maps_value_straight_to_equal_tempered_pitch() {
    // Interface: below 0.5 the values are not quantized; the pitch is
    // low_note + value * range, exact equal temperament, A4 = 440.
    let mut r = rig(serde_json::json!({ "steps": 0.4, "spread": 1.0, "seed": 14 }));
    let steps = r.run(500);
    let mut off_grid = 0;
    for (i, s) in steps.iter().enumerate() {
        let midi = 48.0 + s.value[2] as f64 * 24.0;
        let want = frequency_of(midi);
        assert!(
            (s.freq[2] as f64 / want - 1.0).abs() < 1e-3,
            "step {i}: value {} should give {want} Hz, got {}",
            s.value[2],
            s.freq[2]
        );
        if (midi - midi.round()).abs() > 0.05 {
            off_grid += 1;
        }
    }
    assert!(
        off_grid > 300,
        "unquantized values should fall between notes ({off_grid}/500)"
    );
}

#[test]
fn key_and_scale_controls_retune_a_running_module() {
    let mut r = rig(serde_json::json!({ "spread": 1.0, "steps": 0.5, "seed": 3 }));
    let first = r.run(300);
    assert_all_quantized(&first, 0, &[0, 2, 4, 5, 7, 9, 11], "C major as built");

    r.set("key", "F#".into());
    r.set("scale", "minor_pentatonic".into());
    r.step(); // let the write land
    let second = r.run(600);
    assert_all_quantized(
        &second,
        6,
        &[0, 3, 5, 7, 10],
        "F# minor_pentatonic after control writes",
    );
}

#[test]
fn flat_and_sharp_key_spellings_are_the_same_key() {
    for (flat, sharp) in [
        ("Db", "C#"),
        ("Eb", "D#"),
        ("Gb", "F#"),
        ("Ab", "G#"),
        ("Bb", "A#"),
    ] {
        let config = |key: &str| serde_json::json!({ "key": key, "scale": "major", "spread": 1.0, "seed": 50 });
        let a = rig(config(flat)).run_raw(300);
        let b = rig(config(sharp)).run_raw(300);
        assert_eq!(a, b, "{flat} should behave exactly like {sharp}");
    }
}
