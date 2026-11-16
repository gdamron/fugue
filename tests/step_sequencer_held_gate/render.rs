//! Rendered inventions: the gate a clocked sequencer plays, sample by sample.

use serde_json::json;

use super::rig::{
    assert_high, assert_low, assert_value, clock, dac, document, play, render, rises, runs,
    sequencer, STEP,
};

/// A note then `held`: the gate is high on every sample from
/// the note's attack until the held step's gate ends, on every pass.
#[test]
fn a_note_into_a_held_step_keeps_the_gate_high_across_the_edge() {
    let pattern = json!([{ "note": null }, { "note": 0 }, { "held": true }, { "note": null }]);
    let (gate, edges) = play(pattern, json!({}), "gate", 12);
    for pass in 0..3 {
        let note = edges[4 * pass + 1];
        let after_held = edges[4 * pass + 3];
        assert_high(
            &gate,
            note..after_held - 1,
            &format!("pass {pass}: note through its held step"),
        );
    }
    assert_eq!(
        rises(&gate).len(),
        3,
        "one attack per pass: {:?}",
        runs(&gate)
    );
}

/// Several held steps are one unbroken gate, ending where the last held
/// step's own gate ends (one sample before the next clock edge: a held
/// step's gate lasts its measured step duration counted from its edge, the
/// edge sample included), and the following rests are silent.
#[test]
fn a_run_of_held_steps_is_one_gate_ending_where_the_last_held_gate_ends() {
    let pattern = json!([
        { "note": null },
        { "note": 0 },
        { "held": true },
        { "held": true },
        { "held": true },
        { "note": null },
        { "note": null },
        { "note": null }
    ]);
    // Two whole passes (and half a rest step): no third note starts before
    // the buffer ends, so every run in it is complete.
    let (gate, edges) = play(pattern, json!({}), "gate", 16);
    let expected: Vec<_> = (0..2)
        .map(|pass| edges[8 * pass + 1]..edges[8 * pass + 5] - 1)
        .collect();
    assert_eq!(
        runs(&gate),
        expected,
        "one gate per pass, note through held"
    );
}

/// An envelope driven by the gate does not retrigger at the
/// held step's edge: having decayed to sustain under the note, it stays
/// there instead of attacking again toward full level.
#[test]
fn an_envelope_on_the_gate_does_not_retrigger_at_a_held_step() {
    let pattern = json!([{ "note": null }, { "note": 0 }, { "held": true }, { "note": null }]);
    let json = document(
        vec![
            clock(),
            sequencer(pattern, json!({})),
            json!({ "id": "env", "type": "adsr", "config": {
                "attack": 0.002, "decay": 0.02, "sustain": 0.5, "release": 0.5
            }}),
            dac(),
        ],
        &[
            ["clock", "beat", "seq", "clock"],
            ["seq", "gate", "env", "gate"],
            ["env", "envelope", "dac", "audio_left"],
            ["clock", "beat", "dac", "audio_right"],
        ],
    );
    let (envelope, beat) = render(&json, 8 * STEP + STEP / 2);
    let edges = rises(&beat);
    for pass in 0..2 {
        let held = edges[4 * pass + 2];
        let before = envelope[held - 100];
        assert!(
            (before - 0.5).abs() < 0.01,
            "pass {pass}: the envelope sits at sustain before the held edge, got {before}"
        );
        let peak = envelope[held..held + STEP / 2]
            .iter()
            .copied()
            .fold(f32::MIN, f32::max);
        assert!(
            peak < 0.51,
            "pass {pass}: the envelope attacked again at the held step (peaked at {peak})"
        );
    }
}

/// The velocity output keeps the note's velocity through its
/// held step, after an earlier note at another velocity.
#[test]
fn velocity_keeps_the_note_s_through_its_held_step() {
    let pattern = json!([
        { "note": 0, "velocity": 0.3 },
        { "note": 4, "velocity": 0.6 },
        { "held": true },
        { "note": null }
    ]);
    let (velocity, edges) = play(pattern, json!({}), "velocity", 8);
    for pass in 0..2 {
        let note = edges[4 * pass + 1];
        let after_held = edges[4 * pass + 3];
        assert_value(
            &velocity,
            note + 1..after_held,
            0.6,
            &format!("pass {pass}: the note's velocity"),
        );
    }
}

/// In loop mode a held step 0 continues the note at the end
/// of the pattern across the wrap, unbroken; on the first pass, with no
/// note sounding yet, it is silent like a rest.
#[test]
fn a_held_step_zero_continues_the_last_note_across_the_loop_point() {
    let pattern = json!([{ "held": true }, { "note": null }, { "note": null }, { "note": 0 }]);
    let (gate, edges) = play(pattern, json!({}), "gate", 12);
    assert_low(&gate, 0..edges[3], "the first pass before any note");
    for pass in 0..2 {
        let note = edges[4 * pass + 3];
        let after_wrap = edges[4 * pass + 5];
        assert_high(
            &gate,
            note..after_wrap - 1,
            &format!("pass {pass}: the last note through the wrapped held step 0"),
        );
        assert_low(
            &gate,
            after_wrap..edges[4 * pass + 7],
            &format!("pass {pass}: the rests after the wrapped hold"),
        );
    }
}

/// In one-shot mode there is no wrap: the last
/// note keeps its own gate length and the sequencer falls silent after it.
#[test]
fn one_shot_does_not_wrap_into_a_held_step_zero() {
    let pattern = json!([{ "held": true }, { "note": null }, { "note": null }, { "note": 0 }]);
    let (gate, edges) = play(pattern, json!({ "mode": "one_shot" }), "gate", 8);
    let note = edges[3];
    let run = runs(&gate);
    assert_eq!(run.len(), 1, "one note in the whole playthrough: {run:?}");
    assert_eq!(run[0].start, note);
    assert!(
        run[0].end <= note + STEP / 2 + 1,
        "the last note keeps its half-step gate, got {:?}",
        run[0]
    );
    assert_low(&gate, run[0].end..gate.len(), "after the one-shot ends");
}

/// Two notes in a row at gate_length 1.0 still give the
/// second its own attack: a low sample between them.
#[test]
fn a_note_after_a_full_length_note_still_re_attacks() {
    let pattern = json!([{ "note": null }, { "note": 0 }, { "note": 2 }, { "note": null }]);
    let (gate, edges) = play(pattern, json!({ "gate_length": 1.0 }), "gate", 8);
    for pass in 0..2 {
        let second = edges[4 * pass + 2];
        assert!(
            gate[second - 1] <= 0.5 && gate[second] > 0.5,
            "pass {pass}: the second note re-attacks at its edge {second} \
             (gate {} -> {}; runs {:?})",
            gate[second - 1],
            gate[second],
            runs(&gate)
        );
    }
    assert_eq!(rises(&gate).len(), 4, "two attacks per pass");
}

/// A note not followed by a held step keeps its own
/// gate length: half its measured step here, as on the base commit (the
/// edge sample counts toward it, so the gate falls a sample short).
#[test]
fn a_note_not_followed_by_a_held_step_keeps_its_gate_length() {
    let pattern = json!([{ "note": null }, { "note": 0 }, { "note": null }, { "note": null }]);
    let (gate, edges) = play(pattern, json!({}), "gate", 8);
    let expected: Vec<_> = (0..2)
        .map(|pass| {
            let note = edges[4 * pass + 1];
            let measured = note - edges[4 * pass];
            note..note + (measured as f32 * 0.5) as usize - 1
        })
        .collect();
    assert_eq!(runs(&gate), expected);
}

/// A grace-decorated principal followed by a held step holds
/// unbroken from the principal's attack through the held step, at the
/// principal's velocity (not the grace's 0.8x), under `placement`.
fn assert_grace_principal_holds(pattern: serde_json::Value, placement: &str, note_index: usize) {
    let steps = pattern.as_array().unwrap().len();
    let extra = json!({ "grace_placement": placement });
    let (gate, edges) = play(pattern.clone(), extra.clone(), "gate", 3 * steps);
    let (velocity, _) = play(pattern, extra, "velocity", 3 * steps);
    for pass in 1..3 {
        let step_edge = edges[steps * pass + note_index];
        let after_held = edges[steps * pass + note_index + 2];
        // The principal's attack: the latest rise before mid-step (any grace
        // chain sounds first, before or after the edge).
        let attack = *rises(&gate)
            .iter()
            .rev()
            .find(|&&r| r < step_edge + STEP / 2)
            .unwrap();
        let what = format!("{placement}, pass {pass}");
        assert!(
            attack + STEP / 2 > step_edge,
            "{what}: the principal attacks near its step"
        );
        assert_high(
            &gate,
            attack..after_held - 1,
            &format!("{what}: principal through its held step"),
        );
        assert_value(
            &velocity,
            attack..after_held - 1,
            0.5,
            &format!("{what}: the principal's velocity"),
        );
    }
}

#[test]
fn a_held_step_after_a_grace_before_principal_stays_unbroken() {
    let pattern = json!([
        { "note": null },
        { "note": 7, "velocity": 0.5, "grace": [5] },
        { "held": true },
        { "note": null }
    ]);
    assert_grace_principal_holds(pattern, "before", 1);
}

#[test]
fn a_held_step_after_a_grace_on_beat_deferred_principal_stays_unbroken() {
    let pattern = json!([
        { "note": null },
        { "note": 7, "velocity": 0.5, "grace": [5, 4] },
        { "held": true },
        { "note": null }
    ]);
    assert_grace_principal_holds(pattern, "on_beat", 1);
}

/// A decorated step 0 under `before` placement has nothing to steal from
/// at the wrap, so its principal is deferred behind an on-beat chain.
#[test]
fn a_held_step_after_a_wrapped_deferred_principal_stays_unbroken() {
    let pattern = json!([
        { "note": 7, "velocity": 0.5, "grace": [5] },
        { "held": true },
        { "note": null },
        { "note": null }
    ]);
    assert_grace_principal_holds(pattern, "before", 0);
}
