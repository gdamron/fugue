//! The gain recovers with `release` after a peak has passed.

use super::rig::*;
use super::*;

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
