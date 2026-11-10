use crate::{Noise, RATE};
use fugue::dsp::{Svf, SvfOutput};

fn assert_sums_to_input(svf: &Svf, o: SvfOutput, x: f32, what: &str) {
    let k = 1.0 / svf.q();
    let sum = o.lowpass + k * o.bandpass + o.highpass;
    let scale = 1.0 + o.lowpass.abs() + (k * o.bandpass).abs() + o.highpass.abs();
    assert!(
        (sum - x).abs() <= 1e-5 * scale,
        "{what}: sum {sum}, input {x}"
    );
}

#[test]
fn lowpass_plus_k_bandpass_plus_highpass_equals_input() {
    for (fc, q) in [
        (200.0, 0.707),
        (1000.0, 0.707),
        (1000.0, 2.0),
        (8000.0, 10.0),
        (60.0, 50.0),
        (10_000.0, 100.0),
        (3000.0, 0.1),
    ] {
        let mut svf = Svf::new(RATE, fc, q);
        let mut noise = Noise::new(7);
        for n in 0..9600 {
            let x = match n {
                0 => 1.0,
                _ if n % 1000 == 0 => 0.9,
                _ => 0.6 * noise.sample() + 0.3 * (n as f32 * 0.05).sin(),
            };
            let o = svf.process(x);
            assert_sums_to_input(&svf, o, x, &format!("cutoff {fc}, q {q}, n {n}"));
        }
    }
}

/// The identity holds sample by sample while the settings move.
#[test]
fn the_sum_identity_holds_while_the_settings_change() {
    let mut svf = Svf::new(RATE, 1000.0, 1.0);
    let mut noise = Noise::new(11);
    for n in 0..9600 {
        let t = n as f32;
        svf.set(
            300.0 + 4000.0 * (0.5 + 0.5 * (t * 0.02).sin()),
            0.5 + 20.0 * (0.5 + 0.5 * (t * 0.003).sin()),
        );
        let x = noise.sample();
        let o = svf.process(x);
        assert_sums_to_input(&svf, o, x, &format!("n {n}"));
    }
}

#[test]
fn set_clamps_cutoff_below_nyquist_and_q_to_its_range() {
    let nyquist = RATE as f32 / 2.0;
    let mut svf = Svf::new(RATE, 1000.0, 1.0);
    for fc in [nyquist, nyquist * 1.5, 1e6, 1e12] {
        svf.set(fc, 1.0);
        assert!(
            svf.cutoff() < nyquist && svf.cutoff() > 0.0,
            "set({fc}): cutoff() {}",
            svf.cutoff()
        );
        // Still produces finite output at the clamped cutoff.
        for n in 0..2000 {
            let o = svf.process((n as f32 * 0.4).sin());
            assert!(o.lowpass.is_finite() && o.bandpass.is_finite() && o.highpass.is_finite());
        }
    }
    for q in [0.1, 0.05, 0.0, 1e-9, -3.0] {
        svf.set(1000.0, q);
        assert!((svf.q() - 0.1).abs() < 1e-6, "set q {q}: q() {}", svf.q());
    }
    for q in [100.0, 101.0, 1000.0, 1e9] {
        svf.set(1000.0, q);
        assert!((svf.q() - 100.0).abs() < 1e-3, "set q {q}: q() {}", svf.q());
    }
    svf.set(1000.0, 7.0);
    assert!((svf.q() - 7.0).abs() < 1e-5);
}

#[test]
fn reset_clears_the_state() {
    let mut used = Svf::new(RATE, 800.0, 8.0);
    for n in 0..5000 {
        used.process((n as f32 * 0.1).sin());
    }
    used.reset();
    let mut fresh = Svf::new(RATE, 800.0, 8.0);
    for n in 0..500 {
        let x = (n as f32 * 0.37).sin();
        let (a, b) = (used.process(x), fresh.process(x));
        assert!((a.lowpass - b.lowpass).abs() < 1e-6, "n {n}");
        assert!((a.bandpass - b.bandpass).abs() < 1e-6, "n {n}");
        assert!((a.highpass - b.highpass).abs() < 1e-6, "n {n}");
    }
    let mut quiet = Svf::new(RATE, 800.0, 8.0);
    for n in 0..100 {
        quiet.process((n as f32).sin());
    }
    quiet.reset();
    let o = quiet.process(0.0);
    assert_eq!((o.lowpass, o.bandpass, o.highpass), (0.0, 0.0, 0.0));
}
