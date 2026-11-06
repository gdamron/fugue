//! Criterion 3: the string is in tune from 55 to 1760 Hz.

use super::rig::*;

/// Octaves from 55 to 1760 Hz, then notes between them. Round octaves of 55
/// hide errors that a spectral scan on a bin-aligned grid would miss, so the
/// off-grid frequencies are the ones that make the test non-trivial.
const OCTAVES: [f32; 6] = [55.0, 110.0, 220.0, 440.0, 880.0, 1760.0];
const OFF_GRID: [f32; 6] = [65.41, 123.47, 261.63, 523.25, 987.77, 1567.98];
const TOLERANCE_CENTS: f64 = 5.0;

/// Measures `audio` over 1.5 s that start 0.2 s after the start of the sound.
fn measured(x: &[f32], hz: f32) -> f64 {
    let window = &x[seconds(0.2)..seconds(1.7)];
    assert!(peak(window) > 0.001, "string silent at {hz} Hz");
    cents(peak_near(window, f64::from(hz)), f64::from(hz))
}

fn assert_in_tune(hz: f32, err: f64, what: &str) {
    assert!(
        err.abs() <= TOLERANCE_CENTS,
        "string at {hz} Hz ({what}): fundamental off by {err:.2} cents"
    );
}

/// Criterion: "the string is within +-5 cents from 55 to 1760 Hz, measured on
/// `audio` at default macros", plucked by one trigger at 55-1760 Hz.
/// Assumption: the pitch is the Hann-windowed spectral peak within +-50 cents
/// of the wanted frequency, over the 1.5 s from 0.2 s after the trigger.
#[test]
fn string_pluck_is_in_tune() {
    for hz in OCTAVES.into_iter().chain(OFF_GRID) {
        let mut v = engine("string");
        v.set("frequency", hz);
        let (x, _) = v.render_driven(seconds(1.8), pulses(&[T0]));
        assert_in_tune(hz, measured(&x, hz), "pluck");
    }
}

/// Criterion: the same bound for the string with no trigger patched
/// (continuous excitation), measured over the 1.5 s that start 0.2 s into a
/// render that follows a 0.5 s settling period.
#[test]
fn string_untriggered_is_in_tune() {
    for hz in OCTAVES.into_iter().chain(OFF_GRID) {
        let mut v = engine("string");
        v.set("frequency", hz);
        v.render(seconds(0.5));
        let x = v.render(seconds(1.8));
        assert_in_tune(hz, measured(&x, hz), "un-triggered");
    }
}

/// Criterion: "`frequency` (Hz; overrides the control when connected)": the
/// string follows a connected frequency input to within the same bound while
/// the control sits elsewhere, plucked.
#[test]
fn string_follows_the_frequency_input() {
    for hz in OCTAVES.into_iter().chain(OFF_GRID) {
        let mut v = engine("string");
        v.set("frequency", 300.0);
        let port = v.frequency_in;
        v.hold(port, hz);
        let (x, _) = v.render_driven(seconds(1.8), pulses(&[T0]));
        assert_in_tune(hz, measured(&x, hz), "frequency input");
    }
}

/// Self-check of the measurement: a decaying 440.7 Hz sine, read the same
/// way, is within 0.2 cent, so the +-5 cent tests measure the string and not
/// the rig.
#[test]
fn measurement_reads_a_decaying_sine() {
    let hz = 440.7f64;
    let x: Vec<f32> = (0..seconds(1.8))
        .map(|i| {
            let t = i as f64 / SRF;
            ((std::f64::consts::TAU * hz * t).sin() * (-3.0 * t).exp()) as f32
        })
        .collect();
    let err = cents(peak_near(&x[seconds(0.2)..seconds(1.7)], 440.0), hz);
    assert!(err.abs() < 0.2, "rig error {err} cents");
}
