use crate::{tone_amplitudes, AMP, RATE};
use fugue::dsp::Svf;

/// "Within a few %".
const GAIN_TOLERANCE: f64 = 0.03;

/// Retuning a running filter with `set` gives the same response as building
/// one at those settings.
#[test]
fn set_retunes_a_running_filter() {
    for (fc, q) in [(3000.0, 4.0), (300.0, 0.707), (6000.0, 10.0)] {
        let mut svf = Svf::new(RATE, 1000.0, 1.0);
        // Run it a while at the old settings so it holds state.
        for n in 0..1000 {
            svf.process((n as f32 * 0.3).sin());
        }
        svf.set(fc, q);
        assert!(
            (svf.cutoff() - fc).abs() <= fc * 1e-4,
            "cutoff() {}",
            svf.cutoff()
        );
        assert!((svf.q() - q).abs() <= q * 1e-4, "q() {}", svf.q());
        let (lp, bp, hp) = tone_amplitudes(&mut svf, fc as f64);
        for amp in [lp, bp, hp] {
            assert!(
                (amp / AMP / q as f64 - 1.0).abs() <= GAIN_TOLERANCE,
                "cutoff {fc}, q {q}: gain {}",
                amp / AMP
            );
        }
    }
}
