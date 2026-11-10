//! The `gain` atom: mono gain in dB.

use super::rig::*;
use super::*;

/// Asserts every output over input (where the input is not tiny) is within
/// `tol` of `want(t)`.
fn assert_ratio(
    out: &[f32],
    input: &dyn Fn(usize) -> f32,
    from: usize,
    want: &dyn Fn(usize) -> f64,
    tol: f64,
) {
    for (n, &y) in out.iter().enumerate().skip(from) {
        let x = input(n);
        if x.abs() > 0.01 {
            let r = f64::from(y) / f64::from(x);
            assert!(
                (r - want(n)).abs() <= tol,
                "sample {n}: ratio {r}, want {}",
                want(n)
            );
        }
    }
}

/// Criterion: "`decibels_mod` (dB, audio rate) sums with `decibels` (times
/// `decibels_mod_depth`)", per sample, and the sum clamps to +24 dB.
/// Assumption: the fast dB-to-linear approximation (relative error below
/// 1e-5) keeps every ratio within 1e-4 relative.
#[test]
fn decibels_mod_sums_with_decibels_at_audio_rate() {
    let x = noise(0.4, 6);
    let cases: [(f32, f32, f32, f64); 3] = [
        (-6.0, 2.0, 3.0, 0.0),
        (0.0, 1.0, -20.0, -20.0),
        (24.0, 1.0, 20.0, 24.0),
    ];
    for (decibels, depth, modulation, total) in cases {
        let config = json!({ "decibels": decibels, "decibels_mod_depth": depth });
        let m = move |_| modulation;
        let out = build("gain", config).run(secs(0.1), &[("audio", &x), ("decibels_mod", &m)]);
        let want = db_to_amp(total);
        assert_ratio(&out[0], &x, 256, &|_| want, 1e-4 * want);
    }
    let six = (20.0 * 2f64.log10()) as f32;
    let m = move |t: usize| if t.is_multiple_of(2) { 0.0 } else { six };
    let out = build("gain", json!({})).run(secs(0.1), &[("audio", &x), ("decibels_mod", &m)]);
    assert_ratio(
        &out[0],
        &x,
        0,
        &|n| if n % 2 == 0 { 1.0 } else { 2.0 },
        2e-4,
    );
}

/// A `decibels` change glides over the block and `invert` crossfades (about
/// 20 ms): toggled every block on a 220 Hz sine, neither steps more than the
/// loudest steady sine (plus the glide's own share). Assumption: `invert` is
/// set live as 0 or 1 through the module's numeric control setter.
#[test]
fn decibels_and_invert_changes_do_not_click() {
    let s = sine(0.25, 220.0);
    let slope = 0.25 * std::f64::consts::TAU * 220.0 / f64::from(SR);
    let mut g = build("gain", json!({}));
    g.run(secs(0.1), &[("audio", &s)]);
    let out = g.run_with(secs(1.0), &[("audio", &s)], &mut |t, g| {
        g.set("decibels", if (t / 128) % 2 == 0 { 12.0 } else { 0.0 })
    });
    let limit = (1.25 * db_to_amp(12.0) * slope + 0.25 * 5.0 / 128.0 + 0.002) as f32;
    assert!(
        max_step(&out[0]) <= limit,
        "decibels: step {} > {limit}",
        max_step(&out[0])
    );
    let mut g = build("gain", json!({}));
    g.run(secs(0.1), &[("audio", &s)]);
    let out = g.run_with(secs(1.0), &[("audio", &s)], &mut |t, g| {
        g.set("invert", ((t / secs(0.05)) % 2) as f32)
    });
    let limit = (1.25 * slope + 0.002) as f32;
    assert!(
        max_step(&out[0]) <= limit,
        "invert: step {} > {limit}",
        max_step(&out[0])
    );
}
