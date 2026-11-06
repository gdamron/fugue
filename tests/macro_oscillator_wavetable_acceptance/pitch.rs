//! A2: the `wavetable` fundamental is within +-2 cents.

use super::rig::*;

/// Frames measured for pitch (a power of two, about 1.4 s).
pub const MEASURED: usize = 1 << 16;

pub fn assert_pitch(what: &str, x: &[f32], wanted: f64) {
    assert!(peak(x) > 0.001, "{what}: silent at {wanted} Hz");
    let err = cents(peak_near(x, wanted), wanted);
    assert!(
        err.abs() <= 2.0,
        "{what} at {wanted} Hz: fundamental off by {err:.2} cents"
    );
}

/// Criterion A2: "With nothing patched, `audio`'s fundamental is within
/// +-2 cents of the `frequency` control at 110, 440 and 1760 Hz, at the
/// default macros and at the bank centres (harmonics 0, 1/7, 2/7, 3/7 with
/// timbre and morph 0.5)."
#[test]
fn wavetable_plays_in_tune_at_default_macros_and_bank_centres() {
    let position = wavetable();
    let mut settings: Vec<(String, Vec<(&str, f32)>)> = vec![("default macros".into(), vec![])];
    for bank in 0..4 {
        settings.push((
            format!("bank {bank} centre"),
            vec![
                ("harmonics", bank as f32 / 7.0),
                ("timbre", 0.5),
                ("morph", 0.5),
            ],
        ));
    }
    for (what, macros) in settings {
        for hz in [110.0f32, 440.0, 1760.0] {
            let mut controls = macros.clone();
            controls.push(("frequency", hz));
            let mut v = settled(position, &controls);
            let x = v.render(MEASURED);
            assert_pitch(&format!("wavetable ({what})"), &x, f64::from(hz));
        }
    }
}

/// Self-check of the measurement: synthetic sines are read to within 0.1
/// cent, so the +-2 cent tests measure the module, not the rig.
#[test]
fn measurement_reads_known_sines() {
    for hz in [110.3f64, 440.7, 1761.1] {
        let x: Vec<f32> = (0..MEASURED)
            .map(|i| (TAU_F * hz * i as f64 / SRF).sin() as f32)
            .collect();
        let err = cents(peak_near(&x, hz.round()), hz);
        assert!(err.abs() < 0.1, "rig error {err} cents at {hz} Hz");
    }
}

const TAU_F: f64 = std::f64::consts::TAU;
