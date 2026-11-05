//! Criterion 1: the played pitch is within +-3 cents.

use super::rig::*;
use super::*;

/// Skip the strike transient, then analyse this much of the ring.
const SKIP: f64 = 0.1;
const LENGTH: f64 = 2.0;

fn assert_pitch(model: &str, wanted: f64, mut r: Reso, what: &str) {
    r.render(seconds(SKIP));
    let x = r.render(seconds(LENGTH));
    assert!(peak(&x) > 1e-3, "{model}: silent at {wanted} Hz ({what})");
    let err = cents(peak_near(&x, wanted), wanted);
    assert!(
        err.abs() <= 3.0,
        "{model} at {wanted} Hz ({what}): pitch off by {err:.2} cents"
    );
}

/// Criterion: "Within +-3 cents at 110, 440 and 1760 Hz", for every model,
/// struck once with the internal exciter (`strum` rising edge, `audio`
/// unconnected), default `structure`/`brightness`/`damping`/`position`. The
/// Hann-windowed spectral peak nearest the played pitch (within +-50 cents)
/// is measured on `audio` over the 2 s that follow the first 0.1 s.
/// Assumption: measuring on the mono sum `audio` is as good as on `odd` or
/// `even`; at polyphony 1 the fundamental is present on the sum.
#[test]
fn every_model_plays_in_tune_at_110_440_1760() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        for hz in [110.0f32, 440.0, 1760.0] {
            let mut r = ready(position, hz, 0.5);
            r.strike();
            assert_pitch(
                model,
                f64::from(hz),
                r,
                "internal exciter, frequency control",
            );
        }
    }
}

/// Criterion: "`frequency`: pitch in Hz; while connected it overrides the
/// `frequency` control": a connected 440 Hz input wins over a 220 Hz control,
/// to within +-3 cents, for every model.
#[test]
fn connected_frequency_input_overrides_the_control() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut r = ready(position, 220.0, 0.5);
        r.hold_frequency(440.0);
        r.render(seconds(0.3));
        r.strike();
        assert_pitch(model, 440.0, r, "frequency input over a 220 Hz control");
    }
}

/// Criterion 1 with an external excitation: the pitch is the same when a
/// full-scale one-sample impulse on `audio` excites the body instead of the
/// internal exciter (the "tune anything" use), at 440 Hz for every model.
/// Assumption: with `audio` connected and no strum, the impulse excites the
/// active voice (voice 0 at the start), as INTERFACE.md says.
#[test]
fn external_impulse_sounds_at_the_played_pitch() {
    let names = reso(json!({})).models;
    for (position, model) in names.iter().enumerate() {
        let mut r = ready(position, 440.0, 0.5);
        r.impulse();
        assert_pitch(model, 440.0, r, "impulse on audio");
    }
}

/// Self-check of the measurement: a synthetic 110.4 Hz sine decaying with a
/// 0.5 s time constant is read to within 0.3 cent, so the +-3 cent tests
/// measure the module, not the rig.
#[test]
fn measurement_reads_a_known_decaying_sine() {
    let hz = 110.4f64;
    let x: Vec<f32> = (0..seconds(LENGTH))
        .map(|i| {
            let t = i as f64 / SRF;
            ((std::f64::consts::TAU * hz * t).sin() * (-t / 0.5).exp()) as f32
        })
        .collect();
    let err = cents(peak_near(&x, 110.0), hz);
    assert!(err.abs() < 0.3, "rig error {err} cents");
}
