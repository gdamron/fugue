//! Criterion: "With clock sync on, grain onsets fall on clock divisions
//! +/- 1 sample."
//!
//! Method: constant 0.5 into `audio_left` (the buffer is full of it before
//! the analysis starts), `feedback` 0, `spread` 0, `texture` 0 (near-
//! rectangular window), `size` 0 (about 32 ms grains), a 30 bpm clock (one
//! rising edge every 96,000 samples) so every grain has ended before the
//! next begins even at 8 grains a pulse. A grain's onset is the sample
//! before the output leaves silence (its window is 0 on the onset sample).
//!
//! The divisions are the clock's rising edge plus j x period / n for
//! j = 0..n, with n in {1, 2, 3, 4, 6, 8}. The spec does not say which
//! density gives which n, so n is read back from the output.
//! Assumption: at density below 0.47 every division starts a grain, so the
//! count per pulse is n; above 0.53 grains are at random times, so only that
//! every onset sits on a division is checked.

use super::rig::*;
use super::*;

const PERIOD: usize = 96_000;
const FIRST_EDGE: usize = 1_000;
const PULSES: usize = 10;
const THRESHOLD: f32 = 1e-3;

/// Onsets (the sample before leaving silence) in `out[from..]`.
fn onsets(out: &[f32], from: usize) -> Vec<usize> {
    let mut found = Vec::new();
    let mut quiet = 0;
    for (n, v) in out.iter().enumerate().skip(from) {
        if v.abs() > THRESHOLD {
            if quiet >= 16 {
                found.push(n - 1);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    found
}

fn render_synced(density: f32, sync: bool, clocked: bool) -> Vec<f32> {
    let frames = FIRST_EDGE + (PULSES + 1) * PERIOD;
    let edges = (0..=PULSES).map(|k| FIRST_EDGE + k * PERIOD);
    let feed = Feed::mono(vec![0.5; frames]);
    let feed = if clocked {
        feed.with_clock(gates(frames, edges, 4_800))
    } else {
        feed
    };
    let start = [
        ("mode", GRANULAR),
        ("sync", if sync { 1.0 } else { 0.0 }),
        ("density", density),
        ("size", 0.0),
        ("texture", 0.0),
        ("position", 0.2),
        ("spread", 0.0),
        ("feedback", 0.0),
    ];
    let config = json!({ "buffer_length": 1.0, "seed": 9 });
    render_granular(config, &start, &[], &feed).0
}

/// Distance from `onset` to the nearest division of an `n` grid, over the
/// edges FIRST_EDGE + k x PERIOD.
fn off_grid(onset: usize, n: usize) -> usize {
    let rel = onset as i64 - FIRST_EDGE as i64;
    let step = (PERIOD / n) as i64;
    let nearest = (rel as f64 / step as f64).round() as i64 * step;
    (rel - nearest).unsigned_abs() as usize
}

/// The smallest allowed n whose divisions hold every onset to +/- 1 sample.
fn fitting_n(found: &[usize]) -> Option<usize> {
    [1, 2, 3, 4, 6, 8]
        .into_iter()
        .find(|&n| found.iter().all(|&o| off_grid(o, n) <= 1))
}

/// Densities away from the trigger-only zone (0.47 to 0.53), both sides.
const DENSITIES: [f32; 12] = [0.0, 0.1, 0.2, 0.3, 0.4, 0.45, 0.55, 0.6, 0.7, 0.8, 0.9, 1.0];

#[test]
fn synced_grains_start_on_clock_divisions() {
    let mut n_by_density = Vec::new();
    for density in DENSITIES {
        let out = render_synced(density, true, true);
        // Skip the first two pulses: the period is measured from two edges.
        let found = onsets(&out, FIRST_EDGE + 2 * PERIOD - 100);
        assert!(found.len() >= 2, "density {density}: onsets {found:?}");
        let n = fitting_n(&found).unwrap_or_else(|| {
            panic!("density {density}: onsets are not on a divisions grid: {found:?}")
        });
        if density < 0.47 {
            // Edges 2..=PULSES each start a full pulse of n divisions.
            let want = n * (PULSES - 1);
            assert_eq!(
                found.len(),
                want,
                "density {density}: onsets for n = {n} over {} pulses: {found:?}",
                PULSES - 1
            );
        }
        n_by_density.push((density, n));
    }
    // n = 1 for some density and more than 1 for another, on the regular side.
    let regular: Vec<usize> = n_by_density[..6].iter().map(|x| x.1).collect();
    assert!(
        regular.contains(&1),
        "no density gives n = 1: {n_by_density:?}"
    );
    assert!(
        regular.iter().any(|&n| n > 1),
        "no density gives n > 1: {n_by_density:?}"
    );
    // n grows as density moves away from 0.5 on the regular side.
    assert!(
        regular.windows(2).all(|w| w[0] >= w[1]),
        "n should not shrink moving away from 0.5: {n_by_density:?}"
    );
}

/// "around 0.5 (0.47-0.53) no automatic grains (trigger only)", with and
/// without sync.
#[test]
fn trigger_only_density_starts_no_grains_from_the_clock() {
    for sync in [true, false] {
        for density in [0.47, 0.5, 0.53] {
            let out = render_synced(density, sync, true);
            assert!(
                peak(&out[FIRST_EDGE + PERIOD..]) < 1e-4,
                "density {density}, sync {sync}: output is not silent"
            );
        }
    }
}

/// "Read only while `sync` is on": with sync off, patching a clock changes
/// nothing, bit for bit, while grains still play.
#[test]
fn clock_is_ignored_while_sync_is_off() {
    let (with, without) = (
        render_synced(0.1, false, true),
        render_synced(0.1, false, false),
    );
    assert!(peak(&with) > 0.1, "free-running grains still play");
    assert!(
        with.iter()
            .zip(&without)
            .all(|(a, b)| a.to_bits() == b.to_bits()),
        "the clock input changed the output with sync off"
    );
}
