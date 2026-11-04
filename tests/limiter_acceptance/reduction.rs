//! Criterion: "gain reduction is readable as a control (`reduction`)", and
//! the gain recovers with `release`.

use super::rig::*;
use super::*;

/// Idle (and below the ceiling) `reduction` reads 0; a steady +12 dB sine
/// (peak 4.0) against a -1 dB ceiling reads about 20*log10(4/0.891) = 13.04
/// dB; once the input stops it returns to 0. Assumption: within 0.5 dB.
#[test]
fn reduction_reads_the_gain_reduction_in_db() {
    let loud = sine(4.0, 500.0);
    let quiet = sine(0.5, 500.0);
    let silence = |_| 0.0;
    let mut lim = build("limiter", json!({ "ceiling": -1.0, "release": 0.05 }));
    assert_eq!(lim.get("reduction"), 0.0, "idle before processing");
    lim.run(secs(0.2), &[("audio_left", &quiet)]);
    assert!(lim.get("reduction").abs() < 1e-6, "below the ceiling");
    lim.run(secs(0.5), &[("audio_left", &loud)]);
    let want = 20.0 * (4.0 / db_to_amp(-1.0)).log10();
    let got = f64::from(lim.get("reduction"));
    assert!((got - want).abs() < 0.5, "reduction {got} dB, want {want}");
    lim.run(secs(1.0), &[("audio_left", &silence)]);
    assert!(lim.get("reduction") < 0.1, "back to 0 when idle");
}

/// `reduction` is read-only: writing it is refused.
#[test]
fn writing_reduction_is_refused() {
    let mut lim = build("limiter", json!({}));
    assert!(lim.try_set("reduction", 3.0).is_err());
}

/// Gain (output over delayed input, on a quiet DC level) after a 50 ms burst
/// at +12 dB, sampled `t` seconds after the burst's last sample leaves the
/// output.
fn recovery(release: f64, at: &[f64]) -> Vec<f32> {
    let delay = secs(0.005);
    let end = secs(0.3);
    let sig = move |t: usize| {
        if (secs(0.25)..end).contains(&t) {
            4.0
        } else {
            0.1
        }
    };
    let mut lim = build("limiter", json!({ "ceiling": -1.0, "release": release }));
    let out = lim.run(
        end + delay + secs(5.0 * release + 0.1),
        &[("audio_left", &sig)],
    );
    at.iter()
        .map(|s| out[0][end + delay + secs(*s)] / 0.1)
        .collect()
}

/// "After a peak has passed, the gain recovers 63% of the way back in
/// `release` seconds". Assumption (possible spec gap: linear or dB domain,
/// and whether a hold of up to a lookahead precedes it): checked loosely, as
/// barely recovered at a quarter of `release`, mostly recovered (over 40% of
/// the way, linear) by `release` plus a lookahead, and within 1% of unity
/// after five `release`s plus a lookahead, for 50 ms and 400 ms.
#[test]
fn the_gain_recovers_with_release() {
    let floor = db_to_amp(-1.0) as f32 / 4.0;
    for release in [0.05, 0.4] {
        let g = recovery(
            release,
            &[0.25 * release, release + 0.005, 5.0 * release + 0.005],
        );
        let frac = |g: f32| (g - floor) / (1.0 - floor);
        assert!(
            frac(g[0]) < 0.4,
            "release {release}: {} at a quarter",
            frac(g[0])
        );
        assert!(
            frac(g[1]) > 0.4,
            "release {release}: {} at release",
            frac(g[1])
        );
        assert!(
            g[2] > 0.99 && g[2] <= 1.0 + 1e-6,
            "release {release}: gain {} at 5x",
            g[2]
        );
    }
}
