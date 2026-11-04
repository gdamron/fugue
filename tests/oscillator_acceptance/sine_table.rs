use fugue::dsp::{cosine, sine};
use std::f64::consts::TAU;

/// The table is accurate to 1e-5 for negative phases and phases past one cycle.
/// The reference is evaluated at the exact f32 phase passed in.
#[test]
fn sine_and_cosine_match_f64_reference() {
    let mut worst = 0.0f64;
    for i in 0..8000 {
        let phase = i as f32 * 0.0137 - 50.0; // -50 .. +59.6 cycles
        let p = phase as f64;
        worst = worst.max((sine(phase) as f64 - (TAU * p).sin()).abs());
        worst = worst.max((cosine(phase) as f64 - (TAU * p).cos()).abs());
    }
    assert!(worst < 1e-5, "worst error {worst:e}");
}

#[test]
fn quarter_points_and_whole_cycles() {
    for (phase, s, c) in [(0., 0., 1.), (0.25, 1., 0.), (-0.25, -1., 0.), (3., 0., 1.)] {
        let err = (sine(phase) - s).abs().max((cosine(phase) - c).abs());
        assert!(err < 1e-5, "phase {phase}");
    }
}
