use crate::gain_db;
use fugue::dsp::{OnePole, OnePoleOutput};

const RATE: u32 = 48_000;

fn lowpass_db(rate: u32, fc: f32, f: f64) -> f64 {
    let mut p = OnePole::new(rate, fc);
    gain_db(rate, f, move |x| p.lowpass(x))
}

fn highpass_db(rate: u32, fc: f32, f: f64) -> f64 {
    let mut p = OnePole::new(rate, fc);
    gain_db(rate, f, move |x| p.highpass(x))
}

/// The spec says only "-3 dB at its cutoff"; the tolerance here is 0.5 dB.
#[test]
fn minus_3_db_at_cutoff() {
    for (rate, fc) in [(RATE, 100.0), (RATE, 1e3), (RATE, 5e3), (44_100, 1e3)] {
        let f = fc as f64;
        for g in [lowpass_db(rate, fc, f), highpass_db(rate, fc, f)] {
            assert!((g + 3.0103).abs() <= 0.5, "{rate} Hz, cutoff {fc}: {g} dB");
        }
    }
}

/// One octave apart, well past the cutoff, the gain differs by 6.02 dB (+/- 0.3).
#[test]
fn rolls_off_6_db_per_octave() {
    let lp = lowpass_db(RATE, 50.0, 800.0) - lowpass_db(RATE, 50.0, 1600.0);
    let hp = highpass_db(RATE, 2e3, 125.0) - highpass_db(RATE, 2e3, 62.5);
    assert!((lp - 6.02).abs() <= 0.3, "lowpass {lp} dB/oct");
    assert!((hp - 6.02).abs() <= 0.3, "highpass {hp} dB/oct");
}

/// Assumption: `process`, `lowpass` and `highpass` each advance the same state.
#[test]
fn lowpass_plus_highpass_equals_input() {
    for (rate, fc) in [(RATE, 100.0), (RATE, 3e3), (44_100, 8e3)] {
        let [mut both, mut lp, mut hp] = [(); 3].map(|_| OnePole::new(rate, fc));
        for n in 0..4800 {
            let t = n as f32;
            let click = if n % 997 == 0 { 0.5 } else { 0.0 };
            let x = 0.4 * (t * 0.05).sin() + 0.3 * (t * 1.9).sin() + click;
            let OnePoleOutput { lowpass, highpass } = both.process(x);
            assert!(
                (lowpass + highpass - x).abs() < 1e-5,
                "{rate} Hz, cutoff {fc}, n {n}"
            );
            assert!((lp.lowpass(x) - lowpass).abs() < 1e-6);
            assert!((hp.highpass(x) - highpass).abs() < 1e-6);
        }
    }
}
