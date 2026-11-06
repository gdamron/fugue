//! C2: in tune while droning.

use super::rig::*;

/// Frames measured for pitch (a power of two, about 1.4 s).
const MEASURED: usize = 1 << 16;

/// Criterion C2: "droning (trigger unpatched, timbre 0.5, morph 0.5), the
/// fundamental of `audio` is within +-2 cents of `frequency` at 110, 440 and
/// 880 Hz for patches 1 (soft tine keys), 24 (drawbar organ) and 29
/// (flute)."
/// Assumption: the fundamental is the strongest spectral peak within +-50
/// cents of `frequency` after 0.3 s of settling.
#[test]
fn droning_patches_play_in_tune() {
    for patch in [1usize, 24, 29] {
        for hz in [110.0f32, 440.0, 880.0] {
            let mut v = drone(patch, &[("frequency", hz), ("timbre", 0.5), ("morph", 0.5)]);
            let x = v.render(MEASURED);
            let what = PATCH_NAMES[patch];
            assert!(peak(&x) > 0.001, "{what}: silent at {hz} Hz");
            let err = cents(peak_near(&x, f64::from(hz)), f64::from(hz));
            assert!(
                err.abs() <= 2.0,
                "{what} at {hz} Hz: fundamental off by {err:.2} cents"
            );
        }
    }
}

/// Self-check of the measurement: synthetic sines are read to within 0.1
/// cent, so the +-2 cent test measures the module, not the rig.
#[test]
fn measurement_reads_known_sines() {
    for hz in [110.3f64, 440.7, 881.1] {
        let x: Vec<f32> = (0..MEASURED)
            .map(|i| (std::f64::consts::TAU * hz * i as f64 / SRF).sin() as f32)
            .collect();
        let err = cents(peak_near(&x, hz.round()), hz);
        assert!(err.abs() < 0.1, "rig error {err} cents at {hz} Hz");
    }
}
