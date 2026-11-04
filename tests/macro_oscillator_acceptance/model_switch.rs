//! Criterion 3: changing `model` mid-note does not click.

use super::rig::*;
use super::*;

const BLOCK_SIZE: usize = 128;

/// Largest sample step of `position` playing alone at `hz` for 1 s.
fn steady_max_step(position: usize, hz: f32) -> f32 {
    max_step(&settled(position, hz).render_blocks(seconds(1.0), BLOCK_SIZE))
}

/// Pairs: each adjacent pair of models, and the first model to every other.
fn pairs(count: usize) -> Vec<(usize, usize)> {
    let mut pairs: Vec<(usize, usize)> = (1..count).map(|b| (b - 1, b)).collect();
    pairs.extend((2..count).map(|b| (0, b)));
    pairs
}

fn check_switch(a: usize, b: usize, hz: f32) {
    let names = voice(json!({})).models;
    let bound = 1.5 * steady_max_step(a, hz).max(steady_max_step(b, hz));
    let mut v = settled(a, hz);
    let before = v.render_blocks(seconds(0.1), BLOCK_SIZE);
    v.select(b);
    let after = v.render_blocks(seconds(0.15), BLOCK_SIZE);
    // The step across the switch itself is included via the last old sample.
    let mut window = vec![*before.last().unwrap()];
    window.extend(after);
    let worst = max_step(&window);
    assert!(
        worst <= bound,
        "{} -> {} at {hz} Hz: max step {worst} exceeds 1.5 x steady max ({bound})",
        names[a],
        names[b]
    );
}

/// Criterion: "Changing `model` mid-note: the largest sample-to-sample step
/// of `audio` is no more than 1.5x the steady-state maximum of either
/// engine", over the switch and the following 150 ms (>= the ~100 ms asked),
/// at 440 Hz and at 110 Hz. With one model present this passes vacuously.
/// Assumption: the switch lands on a 128-frame block boundary, and the
/// steady-state maximum is taken over 1 s after a 0.3 s settle.
#[test]
fn switching_model_mid_note_does_not_click() {
    let count = voice(json!({})).models.len();
    if count < 2 {
        eprintln!("vacuous: only {count} model present, nothing to switch between");
        return;
    }
    for (a, b) in pairs(count) {
        for hz in [440.0, 110.0] {
            check_switch(a, b, hz);
        }
    }
}
