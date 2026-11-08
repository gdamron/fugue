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

/// Criterion: "controls are declared": ports and the pinned controls.
#[test]
fn gain_declares_its_ports_and_controls() {
    let g = build("gain", json!({}));
    for port in ["audio", "decibels_mod"] {
        assert!(g.m().input_port_index(port).is_some(), "input {port}");
    }
    assert_eq!(g.m().outputs(), ["audio"]);
    assert_number(&g, "decibels", -96.0, 24.0, 0.0);
    assert_number(&g, "decibels_mod_depth", 0.0, 96.0, 1.0);
    let invert = g.meta("invert");
    assert_eq!(
        (invert.kind, invert.default),
        (ControlKind::Bool, ControlValue::Bool(false))
    );
    assert!(
        try_build("gain", json!({ "no_such_key": 1 })).is_err(),
        "config is closed"
    );
}

/// Criterion: "0 dB is a bit-identical passthrough".
#[test]
fn zero_db_is_a_bit_identical_passthrough() {
    let x = noise(1.5, 9);
    let out = build("gain", json!({})).run(secs(0.3), &[("audio", &x)]);
    for (n, y) in out[0].iter().enumerate() {
        assert_eq!(y.to_bits(), x(n).to_bits(), "sample {n}");
    }
}

/// Criterion: "+6.0206 dB (20*log10 2) is x2 within 1e-4", set from config
/// and live (after the change's glide, one block).
#[test]
fn plus_6_0206_db_doubles() {
    let x = noise(0.4, 5);
    let db = (20.0 * 2f64.log10()) as f32;
    let mut g = build("gain", json!({ "decibels": db }));
    assert!((g.get("decibels") - db).abs() < 1e-6, "starts from config");
    let out = g.run(secs(0.1), &[("audio", &x)]);
    assert_ratio(&out[0], &x, 256, &|_| 2.0, 1e-4);
    let mut g = build("gain", json!({}));
    g.run(secs(0.05), &[("audio", &x)]);
    g.set("decibels", db);
    let t0 = g.t;
    let out = g.run(secs(0.1), &[("audio", &x)]);
    assert_ratio(&out[0], &|n| x(n + t0), 256, &|_| 2.0, 1e-4);
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

/// Criterion: "`invert` flips sign".
#[test]
fn invert_flips_the_sign() {
    let x = noise(0.7, 8);
    let mut g = build("gain", json!({ "invert": true, "decibels": 6.0 }));
    let out = g.run(secs(0.2), &[("audio", &x)]);
    assert_ratio(&out[0], &x, secs(0.1), &|_| -db_to_amp(6.0), 1e-4);
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
