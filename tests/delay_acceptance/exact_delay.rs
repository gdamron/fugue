//! Criterion 1: with `time` = N/sr and feedback 0 the atom delays by exactly
//! N samples, bit for bit within +/-0.75. Also the time clamp, `time_mod` and
//! non-finite input.

use super::rig::*;
use super::*;

fn assert_delayed_by(n: usize, time: f32, block: usize) {
    let input = noise(n + 6000, 0.75, 11);
    let out = delay(json!({ "time": time, "feedback": 0.0 })).run(&input, block, |_, _| {});
    assert!(
        out[..n].iter().all(|&v| v == 0.0),
        "N={n}: silence before the first sample"
    );
    let wrong = (0..input.len() - n).find(|&i| out[i + n].to_bits() != input[i].to_bits());
    assert!(
        wrong.is_none(),
        "N={n} block={block}: output differs from input at {wrong:?}"
    );
}

/// Criterion 1 with times that are exact in f32 (N/sr is dyadic).
#[test]
fn time_of_n_over_sr_delays_by_exactly_n_samples() {
    for (n, time) in [
        (375, 0.0078125f32),
        (1500, 0.03125),
        (12_000, 0.25),
        (24_000, 0.5),
    ] {
        assert_delayed_by(n, time, BLOCK);
    }
}

/// Criterion 1 for any whole N, including N below the block size, with the
/// control given as the f32 nearest N/sr. Assumption: the atom treats a time
/// that is a whole number of samples to within f32 resolution as exact (the
/// interface says "a whole number of samples is exact").
#[test]
fn time_of_arbitrary_n_over_sr_delays_by_exactly_n_samples() {
    for n in [1usize, 7, 100, 997, 4_321] {
        for block in [64, 128, 1000] {
            assert_delayed_by(n, n as f32 / SR as f32, block);
        }
    }
}

/// Criterion 1 on the live path: noise -> delay -> dac in an invention, the
/// delayed output equal to the dry output N samples earlier.
#[test]
fn invention_delay_matches_dry_signal_n_samples_earlier() {
    let mut engine = engine(tap_doc(
        noise_source(0.7),
        "white",
        json!({ "time": 0.03125 }),
    ));
    let (dry, wet) = render(&mut engine, 96_000);
    // The first 0.1 s is skipped in case the source's level is still settling.
    let bad = (4_800..dry.len() - 1500).find(|&i| wet[i + 1500].to_bits() != dry[i].to_bits());
    assert!(
        bad.is_none(),
        "wet differs from dry 1500 samples earlier at {bad:?}"
    );
}

/// `time` is clamped to `max_time` (and `max_time` bounds the line).
#[test]
fn time_is_clamped_to_max_time() {
    let mut d = delay(json!({ "max_time": 0.1, "time": 1.0 }));
    let out = d.render(&impulses(12_000, &[(100, 0.5)]));
    assert_hits(&out, &[(100 + 4_800, 0.5)]);
}

/// `time_mod` adds `time_mod x time_mod_depth` seconds, at audio rate.
#[test]
fn time_mod_adds_depth_seconds_of_delay() {
    let mut d = delay(json!({ "time": 0.0625, "time_mod_depth": 0.01 }));
    d.mod_in = 1.0;
    let out = d.render(&impulses(12_000, &[(100, 0.5)]));
    assert_hits(&out, &[(100 + 3_000 + 480, 0.5)]);
}

/// "Non-finite input is recorded as silence": the output stays finite, and
/// the module recovers.
#[test]
fn non_finite_input_is_recorded_as_silence() {
    let mut input = noise(20_000, 0.5, 5);
    input[100] = f32::NAN;
    input[101] = f32::INFINITY;
    input[102] = f32::NEG_INFINITY;
    let out = delay(json!({ "time": 0.0625, "feedback": 0.9 })).render(&input);
    assert!(out.iter().all(|v| v.is_finite()), "output stays finite");
    assert!(peak(&out[10_000..]) > 0.1, "later audio still passes");
}

/// The atom refuses unknown config keys and a `max_time` of 0 or less; a
/// `max_time` above 60 s builds and clamps to 60 s (`time` is clamped to
/// `max_time`, so a `time` of 60.5 reads back at most 60).
#[test]
fn config_is_closed_and_max_time_is_bounded() {
    assert!(try_delay(json!({ "no_such_key": 1.0 })).is_err());
    assert!(try_delay(json!({ "max_time": 0.0 })).is_err());
    let config = json!({ "max_time": 60.5, "time": 60.5 });
    let built = ModuleRegistry::default().build("delay", SR, &config);
    let built = built.unwrap_or_else(|err| panic!("max_time above 60 builds: {err}"));
    let time = built
        .module
        .module()
        .get_control("time")
        .expect("time reads back");
    assert!(time <= 60.0, "time {time} exceeds the 60 s ceiling");
    assert!(try_delay(json!({ "max_time": 60.0 })).is_ok());
}
