use crate::{db, tone_amplitudes, AMP, RATE};
use fugue::dsp::Svf;

const QS: [f32; 3] = [0.707, 2.0, 10.0];
const CUTOFFS: [f32; 6] = [200.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

/// "Within a few %".
const GAIN_TOLERANCE: f64 = 0.03;

/// At the cutoff the lowpass, bandpass and highpass gains each equal q.
#[test]
fn every_output_has_gain_q_at_the_cutoff() {
    for q in QS {
        for fc in CUTOFFS {
            let mut svf = Svf::new(RATE, fc, q);
            let (lp, bp, hp) = tone_amplitudes(&mut svf, fc as f64);
            for (name, amp) in [("lowpass", lp), ("bandpass", bp), ("highpass", hp)] {
                let gain = amp / AMP;
                assert!(
                    (gain / q as f64 - 1.0).abs() <= GAIN_TOLERANCE,
                    "{name}, q {q}, cutoff {fc} Hz: gain {gain}"
                );
            }
        }
    }
}

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

/// Two octaves-plus above the cutoff the lowpass loses 12 dB per octave. Frequencies
/// stay low enough in the band that the bilinear transform's warping is small.
#[test]
fn lowpass_falls_12_db_per_octave_above_the_cutoff() {
    for q in [0.707, 1.0, 2.0, 5.0] {
        for (fc, f) in [(100.0, 1000.0), (200.0, 2000.0), (200.0, 3000.0)] {
            let gain = |f: f64| {
                let mut svf = Svf::new(RATE, fc, q);
                db(tone_amplitudes(&mut svf, f).0 / AMP)
            };
            let drop = gain(f) - gain(2.0 * f);
            assert!(
                (drop - 12.0).abs() <= 1.0,
                "q {q}, cutoff {fc}: {drop} dB from {f} to {} Hz",
                2.0 * f
            );
        }
    }
}

/// Well below the cutoff the highpass rises 12 dB per octave.
#[test]
fn highpass_rises_12_db_per_octave_below_the_cutoff() {
    for q in [0.707, 1.0, 2.0, 5.0] {
        for (fc, f) in [(4000.0, 200.0), (4000.0, 400.0), (2000.0, 100.0)] {
            let gain = |f: f64| {
                let mut svf = Svf::new(RATE, fc, q);
                db(tone_amplitudes(&mut svf, f).2 / AMP)
            };
            let rise = gain(2.0 * f) - gain(f);
            assert!(
                (rise - 12.0).abs() <= 1.0,
                "q {q}, cutoff {fc}: {rise} dB from {f} to {} Hz",
                2.0 * f
            );
        }
    }
}

/// The lowpass passes low tones and the highpass blocks them, and the reverse
/// at the top: a sanity check that the outputs are not swapped.
#[test]
fn lowpass_passes_lows_and_highpass_passes_highs() {
    let mut svf = Svf::new(RATE, 2000.0, 0.707);
    let (lp, _, hp) = tone_amplitudes(&mut svf, 100.0);
    assert!(
        (lp / AMP - 1.0).abs() < 0.02 && hp / AMP < 0.01,
        "lp {lp}, hp {hp}"
    );
    let mut svf = Svf::new(RATE, 2000.0, 0.707);
    let (lp, _, hp) = tone_amplitudes(&mut svf, 16_000.0);
    assert!(
        (hp / AMP - 1.0).abs() < 0.1 && lp / AMP < 0.02,
        "lp {lp}, hp {hp}"
    );
}
