//! What the `velocity` output holds, read sample by sample with the output
//! sent straight to the dac, against the sequencer's gate and the clock's
//! beat: a step's velocity from its onset, 1.0 for a step that declares
//! none, and 0.8 of its step's velocity for a grace note.

use serde_json::json;

use super::rig::*;

/// The sequencer's velocity and gate, and the clock's beat edges, over
/// `frames` frames. The gate and beat come from a second render of the same
/// deterministic patch.
fn read(config: serde_json::Value, frames: usize) -> (Vec<f32>, Vec<f32>, Vec<usize>) {
    let (velocity, gate) = render(&probe(config.clone(), "velocity", ("seq", "gate")), frames);
    let (gate_again, beat) = render(&probe(config, "gate", ("clock", "beat")), frames);
    assert_eq!(gate, gate_again, "the patch renders alike twice");
    (velocity, gate, rises(&beat))
}

#[test]
fn velocity_is_the_latest_attack_s_and_holds_through_release_rests_and_held_steps() {
    let config = json!({
        "root_note": 60, "step_count": 8, "gate_length": 0.5,
        "pattern": [
            { "note": null },
            { "note": 0, "velocity": 0.3 },
            { "note": null },
            { "note": 2, "velocity": 0.6 },
            { "held": true },
            { "note": null },
            { "note": 4 },
            { "note": 5, "velocity": 0.45 }
        ]
    });
    let (velocity, gate, edges) = read(config, 18 * STEP);
    assert!(edges.len() >= 17, "two loops of the pattern: {edges:?}");

    // What the output holds over each step: 1.0 before any note has played,
    // a step's own velocity from its onset, the latest attack's through its
    // release, a rest and a held step, 1.0 for a step that declares none, and
    // at the second loop's rest the last note's 0.45 rather than 1.0 again.
    let holds = [1.0, 0.3, 0.3, 0.6, 0.6, 0.6, 1.0, 0.45];
    for (i, &value) in velocity.iter().enumerate().take(edges[0]) {
        assert_near(value, 1.0, &format!("sample {i}, before the first step"));
    }
    for k in 0..16 {
        let want = if k == 8 { 0.45 } else { holds[k % 8] };
        let span = edges[k]..edges[k + 1];
        for (i, &value) in velocity.iter().enumerate().take(span.end).skip(span.start) {
            assert_near(
                value,
                want,
                &format!("step {k} (loop step {}), sample {i}", k % 8),
            );
        }
    }
    assert_changes_only_at_onsets(&velocity, &gate, "plain steps");
}

/// Each grace note at 0.8 times its step's velocity, the principal at the
/// step's own, under `placement`.
fn grace_notes_scale_their_step_s_velocity(placement: &str) {
    let config = json!({
        "root_note": 60, "step_count": 4, "gate_length": 0.5,
        "grace_placement": placement,
        "pattern": [
            { "note": null },
            { "note": 0, "velocity": 0.5 },
            { "note": 4, "velocity": 0.9, "grace": [2, 3] },
            { "note": 7, "grace": [5] }
        ]
    });
    let (velocity, gate, edges) = read(config, 10 * STEP);
    assert!(edges.len() >= 9, "two loops of the pattern: {edges:?}");
    let runs = runs(&gate);

    // From step 1 to the next loop's step 1 the gate opens six times: the
    // plain note, two graces, their principal, one grace, its principal.
    // Under `before` the graces take the previous step's tail; under
    // `on_beat` they start on the step and delay the principal. Either way
    // the attacks come in this order.
    let want = [0.5, 0.8 * 0.9, 0.8 * 0.9, 0.9, 0.8, 1.0];
    for (loop_index, window) in [edges[1]..edges[5], edges[5]..edges[9]].iter().enumerate() {
        let attacks: Vec<_> = runs
            .iter()
            .filter(|run| window.contains(&run.start))
            .collect();
        assert_eq!(
            attacks.len(),
            want.len(),
            "{placement}, loop {loop_index}: attacks {attacks:?}"
        );
        for (n, (run, want)) in attacks.iter().zip(want).enumerate() {
            for i in (*run).clone() {
                assert_near(
                    velocity[i],
                    want,
                    &format!("{placement}, loop {loop_index}, attack {n}, sample {i}"),
                );
            }
        }
    }
    assert_changes_only_at_onsets(&velocity, &gate, placement);
}

#[test]
fn grace_notes_sound_at_eight_tenths_of_their_step_s_velocity_before_the_beat() {
    grace_notes_scale_their_step_s_velocity("before");
}

#[test]
fn grace_notes_sound_at_eight_tenths_of_their_step_s_velocity_on_the_beat() {
    grace_notes_scale_their_step_s_velocity("on_beat");
}
