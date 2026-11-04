//! Finite, bounded, stationary output over long renders.

use super::rig::*;

/// Criterion: "outputs stay finite and bounded (pink should not run away
/// over long renders)": 60 s at level 1 is finite everywhere, with white
/// peaking within +/-1 (uniform white with RMS 0.577 fills +/-1) and pink
/// under a generous runaway bound of 8.
/// Assumption: a bound of 8 on pink is a runaway detector, not a level spec;
/// pink's absolute level is not specified.
#[test]
fn sixty_seconds_stay_finite_and_bounded() {
    let (white, pink) = seeded(7).render(seconds(60.0));
    assert!(
        white.iter().chain(&pink).all(|v| v.is_finite()),
        "non-finite sample"
    );
    assert!(peak(&white) <= 1.0 + 1e-6, "white peak {}", peak(&white));
    assert!(peak(&pink) <= 8.0, "pink peak {}", peak(&pink));
}

/// Criterion: pink does not drift or decay: each 10 s window's RMS is within
/// a factor of two of the whole run's, and pink is audible (RMS above 0.01).
#[test]
fn pink_level_is_stationary_over_a_minute() {
    let (_, pink) = seeded(7).render(seconds(60.0));
    let total = rms(&pink);
    assert!(total > 0.01, "pink rms {total}");
    for (i, window) in pink.chunks(seconds(10.0)).enumerate() {
        let r = rms(window);
        assert!(
            r > 0.5 * total && r < 2.0 * total,
            "window {i}: rms {r} vs {total}"
        );
    }
}

/// Criterion: same bounds hold with a different seed and a block size
/// that is not a power of two, to exercise state carried across blocks.
#[test]
fn bounded_with_another_seed_and_odd_blocks() {
    let (white, pink) = seeded(0xFEED).render_blocks(seconds(20.0), 333);
    assert!(white.iter().chain(&pink).all(|v| v.is_finite()));
    assert!(peak(&white) <= 1.0 + 1e-6 && peak(&pink) <= 8.0);
}
