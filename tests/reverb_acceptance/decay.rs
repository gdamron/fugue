//! Criterion 1: RT60 follows the decay setting.

use super::rig::*;

/// RT60 from an impulse response by Schroeder backward integration and a
/// T20 line fit (-5 dB to -25 dB), extrapolated to 60 dB.
fn rt60_t20(ir: &[f32]) -> f64 {
    let mut edc = vec![0.0f64; ir.len()];
    let mut acc = 0.0;
    for i in (0..ir.len()).rev() {
        acc += (ir[i] as f64).powi(2);
        edc[i] = acc;
    }
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &e) in edc.iter().enumerate() {
        let db = 10.0 * (e / edc[0]).log10();
        if (-25.0..=-5.0).contains(&db) {
            let t = i as f64 / SRF;
            n += 1.0;
            sx += t;
            sy += db;
            sxx += t * t;
            sxy += t * db;
        }
    }
    assert!(n > 100.0, "decay never reaches the -5..-25 dB fit range");
    let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
    -60.0 / slope
}

/// Impulse response (left + right) of a wet-only reverb to a mono unit impulse.
fn impulse_response(overrides: &[(&str, f64)], seconds: f64) -> Vec<f32> {
    let mut m = wet_module(overrides);
    let (l, r) = render(
        m.as_mut(),
        secs(seconds),
        BLOCK,
        |i| (if i == 0 { 1.0 } else { 0.0 }, 0.0),
        false,
        no_hook,
    );
    assert!(all_finite(&l) && all_finite(&r));
    l.iter().zip(&r).map(|(a, b)| a + b).collect()
}

fn target_rt60(decay: f64) -> f64 {
    0.2 + 14.8 * decay
}

fn assert_within(measured: f64, target: f64, tol: f64, what: &str) {
    println!("{what}: measured {measured:.3}, target {target:.3}");
    assert!(
        (measured - target).abs() <= tol * target,
        "{what}: measured {measured:.3} s vs target {target:.3} s (tolerance {:.0}%)",
        tol * 100.0
    );
}

/// Second difference: a crude high-pass that keeps mostly the top octaves.
fn high_band(x: &[f32]) -> Vec<f32> {
    (2..x.len())
        .map(|i| x[i] - 2.0 * x[i - 1] + x[i - 2])
        .collect()
}

fn check_rt60_at(decay: f64) {
    let target = target_rt60(decay);
    let ir = impulse_response(&[("decay", decay), ("damping", 0.0)], target + 0.5);
    assert_within(rt60_t20(&ir), target, 0.25, &format!("decay {decay}"));
}

/// Criterion 1: RT60 at decay 0.1 is within 25% of 0.2 + 14.8 * decay.
#[test]
fn rt60_follows_decay_setting_short() {
    check_rt60_at(0.1);
}

/// Criterion 1: RT60 at decay 0.3 is within 25% of 0.2 + 14.8 * decay.
#[test]
fn rt60_follows_decay_setting_medium() {
    check_rt60_at(0.3);
}

/// Criterion 1: RT60 at decay 0.6 is within 25% of 0.2 + 14.8 * decay.
#[test]
fn rt60_follows_decay_setting_long() {
    check_rt60_at(0.6);
}

/// Criterion 1: RT60 is a property of decay, not of room_size.
#[test]
fn rt60_is_independent_of_room_size() {
    let target = target_rt60(0.3);
    for room in [0.1, 0.9, 1.0] {
        let ir = impulse_response(
            &[("decay", 0.3), ("damping", 0.0), ("room_size", room)],
            target + 0.5,
        );
        assert_within(rt60_t20(&ir), target, 0.25, &format!("room {room}"));
    }
}

/// Criterion 1: with damping = 0 the decay follows the RT60 broadband, so the
/// top octaves decay as fast as the whole signal; damping = 1 makes them
/// decay clearly faster.
#[test]
fn damping_zero_is_broadband_and_damping_shortens_highs() {
    let target = target_rt60(0.3);
    let flat = impulse_response(&[("decay", 0.3), ("damping", 0.0)], target + 0.5);
    let hi_flat = rt60_t20(&high_band(&flat));
    assert_within(hi_flat, target, 0.25, "damping 0, high band");
    let damped = impulse_response(&[("decay", 0.3), ("damping", 1.0)], target + 0.5);
    let hi_damped = rt60_t20(&high_band(&damped));
    println!("damping 1, high band: {hi_damped:.3}");
    assert!(
        hi_damped < 0.7 * hi_flat,
        "damping 1 high band {hi_damped:.3} s should be well under undamped {hi_flat:.3} s"
    );
}
