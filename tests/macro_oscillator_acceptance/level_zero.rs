//! Criterion 4: at level 0 the output is below -80 dBFS.

use super::rig::*;
use super::*;

/// -80 dBFS as a peak amplitude.
const FLOOR: f32 = 1e-4;

fn names() -> Vec<String> {
    voice(json!({})).models
}

fn assert_silent(model: &str, what: &str, x: &[f32]) {
    // Skip the first 50 ms.
    let p = peak(&x[seconds(0.05)..]);
    assert!(p < FLOOR, "{model} ({what}): peak {p} is above -80 dBFS");
}

/// Control: the same setup at full level is audible, so silence below is a
/// property of level 0 and not of a dead module.
fn assert_audible_at_full_level(position: usize, model: &str) {
    let mut v = settled(position, 440.0);
    let port = v.level_in;
    v.hold(port, 1.0);
    let p = peak(&v.render(seconds(0.3)));
    assert!(p > 0.01, "{model}: peak {p} at level input 1, not audible");
}

/// Criterion: "connect the `level` input at 0 and check that `audio` (after
/// an initial ~50 ms) has a peak below -80 dBFS for each model".
#[test]
fn level_input_at_zero_silences_every_model() {
    for (position, model) in names().iter().enumerate() {
        assert_audible_at_full_level(position, model);
        let mut v = voice(json!({}));
        v.select(position);
        let port = v.level_in;
        v.hold(port, 0.0);
        assert_silent(model, "level input 0", &v.render(seconds(0.5)));
    }
}

/// Criterion: "Also test the `level` control at 0 with the input
/// unconnected", set while the voice is sounding, then 50 ms to settle.
#[test]
fn level_control_at_zero_silences_every_model() {
    for (position, model) in names().iter().enumerate() {
        let mut v = settled(position, 440.0);
        v.set("level", 0.0);
        assert_silent(model, "level control set to 0", &v.render(seconds(0.5)));
    }
}

/// Criterion: level 0 silences the gate "at level 0, the gate output is below
/// -80 dBFS". Assumption: trigger edges ping the gate but do not lift it
/// above a level of 0, so triggers every 100 ms at level input 0 stay silent.
#[test]
fn triggers_do_not_open_a_gate_at_level_zero() {
    for (position, model) in names().iter().enumerate() {
        let mut v = voice(json!({}));
        v.select(position);
        let (level, trigger) = (v.level_in, v.trigger_in);
        v.hold(level, 0.0);
        v.connect(trigger);
        let mut x = Vec::new();
        for _ in 0..(seconds(0.5) / BLOCK) {
            v.input_block(trigger)[..BLOCK].fill(0.0);
            v.input_block(trigger)[0] = 1.0;
            v.process(BLOCK);
            x.extend_from_slice(v.audio(BLOCK));
        }
        assert_silent(model, "triggered at level 0", &x);
    }
}
