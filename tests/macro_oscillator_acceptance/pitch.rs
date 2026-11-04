//! Criterion 1: each engine's fundamental is within +-2 cents.

use super::rig::*;
use super::*;

const LENGTH: f64 = 4.0;

fn assert_pitch(model: &str, wanted: f64, mut v: Voice, what: &str) {
    let x = v.render(seconds(LENGTH));
    assert!(peak(&x) > 0.001, "{model}: silent at {wanted} Hz ({what})");
    let err = cents(peak_near(&x, wanted), wanted);
    assert!(
        err.abs() <= 2.0,
        "{model} at {wanted} Hz ({what}): fundamental off by {err:.2} cents"
    );
}

/// Criterion: "Each engine at 110, 440 and 1760 Hz has a fundamental within
/// +-2 cents, at default macro settings", measured on `audio` after 0.3 s.
/// Assumption: with `trigger` and `level` unconnected the gate is open.
#[test]
fn every_model_plays_in_tune_at_110_440_1760() {
    let names = voice(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        for hz in [110.0f32, 440.0, 1760.0] {
            assert_pitch(
                model,
                f64::from(hz),
                settled(position, hz),
                "frequency control",
            );
        }
    }
}

/// Criterion: "`frequency` (Hz; when connected it overrides the control, as
/// on `oscillator`)": a connected 440 Hz input wins over a 220 Hz control,
/// to within +-2 cents, for every model.
#[test]
fn connected_frequency_input_overrides_the_control() {
    let names = voice(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut v = settled(position, 220.0);
        let port = v.frequency_in;
        v.hold(port, 440.0);
        v.render(seconds(0.3));
        assert_pitch(model, 440.0, v, "frequency input");
    }
}

/// Criterion: "`frequency_mod_depth`: Hz of FM per unit of the
/// `frequency_mod` input". Assumption: a constant `frequency_mod` of 1 with
/// depth 100 raises a 440 Hz voice by a steady 100 Hz (to 540 Hz), i.e. the
/// offset is linear in Hz and the input is not scaled by anything else.
#[test]
fn frequency_mod_offsets_pitch_by_depth_in_hz() {
    let names = voice(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut v = settled(position, 440.0);
        v.set("frequency_mod_depth", 100.0);
        let port = v.frequency_mod_in;
        v.hold(port, 1.0);
        v.render(seconds(0.3));
        assert_pitch(model, 540.0, v, "frequency_mod 1 x depth 100");
    }
}

/// Self-check of the measurement: a synthetic 440.7 Hz sine is read to
/// within 0.1 cent, so the +-2 cent tests measure the module, not the rig.
#[test]
fn measurement_reads_a_known_sine() {
    let hz = 440.7f64;
    let x: Vec<f32> = (0..seconds(LENGTH))
        .map(|i| (std::f64::consts::TAU * hz * i as f64 / SRF).sin() as f32)
        .collect();
    let err = cents(peak_near(&x, 440.0), hz);
    assert!(err.abs() < 0.1, "rig error {err} cents");
}
