//! Seeded determinism.

use super::rig::*;

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

/// Criterion: bit-identical across a long render, so a drifting or
/// state-leaking generator fails late rather than early.
#[test]
fn same_seed_stays_identical_over_ten_seconds() {
    let (w1, p1) = seeded(42).render(seconds(10.0));
    let (w2, p2) = seeded(42).render(seconds(10.0));
    assert_eq!(bits(&w1), bits(&w2));
    assert_eq!(bits(&p1), bits(&p2));
}

/// Criterion (reasonably): "different seeds give different output". Almost
/// every sample differs and the two streams are uncorrelated.
#[test]
fn different_seeds_give_different_output() {
    let (w1, p1) = seeded(1).render(seconds(1.0));
    let (w2, p2) = seeded(2).render(seconds(1.0));
    for (name, a, b) in [("white", &w1, &w2), ("pink", &p1, &p2)] {
        let differing = a.iter().zip(b).filter(|(x, y)| x != y).count();
        assert!(
            differing > a.len() * 99 / 100,
            "{name}: only {differing} of {} samples differ",
            a.len()
        );
        let dot: f64 = a
            .iter()
            .zip(b)
            .map(|(x, y)| f64::from(*x) * f64::from(*y))
            .sum();
        let corr = dot / (rms(a) * rms(b) * a.len() as f64);
        assert!(corr.abs() < 0.05, "{name}: seeds 1 and 2 correlate {corr}");
    }
}

/// Criterion: `seed` is an unsigned integer, so every value including 0 and
/// the maximum is a valid seed that yields live noise (a generator that
/// locks up on a zero seed fails).
#[test]
fn edge_seeds_produce_noise() {
    for seed in [0u64, 1, u64::MAX] {
        let (white, pink) = seeded(seed).render(seconds(1.0));
        assert!(rms(&white) > 0.5, "seed {seed}: white rms {}", rms(&white));
        assert!(rms(&pink) > 0.01, "seed {seed}: pink rms {}", rms(&pink));
    }
}

/// Assumption (not a spec criterion): output does not depend on how the
/// render is chunked into blocks, so offline and live renders of one seed
/// agree.
#[test]
fn output_is_independent_of_block_size() {
    let (w1, p1) = seeded(7).render_blocks(seconds(1.0), 64);
    let (w2, p2) = seeded(7).render_blocks(seconds(1.0), 1000);
    assert_eq!(bits(&w1), bits(&w2), "white depends on block size");
    assert_eq!(bits(&p1), bits(&p2), "pink depends on block size");
}
