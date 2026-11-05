//! Criterion: "A buffer of length L plays seamlessly across its wrap point."
//!
//! Method: a 100 Hz sine at 0.4 (480 samples a cycle) is recorded into a
//! 0.2 s buffer, exactly 20 cycles, so the recorded signal is continuous
//! across the buffer's end and start whatever offset is read. Playback must
//! then be as smooth as the same module with a 4 s buffer (no wrap in the
//! compared window): largest step within 1.5x, no gap, no level loss.
//! Granular mode is driven by a trigger every 0.25 s (25 cycles, so
//! overlapping grains stay in phase) with density in its trigger-only zone.

use super::rig::*;
use super::*;

const SMALL: f64 = 0.2;
const LARGE: f64 = 4.0;

fn feed(mode: f32, frames: usize) -> Feed {
    let input = sine(frames, 100.0, 0.4);
    if mode == GRANULAR {
        let edges = (0..)
            .map(|k| s(0.1) + k * s(0.25))
            .take_while(|&e| e < frames);
        Feed::mono(input).with_trigger(gates(frames, edges, 48))
    } else {
        Feed::mono(input)
    }
}

fn controls(mode: f32) -> Vec<(&'static str, f32)> {
    vec![
        ("mode", mode),
        ("density", 0.5),
        ("size", 1.0),
        ("position", 0.5),
    ]
}

fn render_with(length: f64, mode: f32, frames: usize, freeze_at: Option<f64>) -> Vec<f32> {
    let config = json!({ "buffer_length": length, "seed": 5 });
    let events: Vec<Event> = freeze_at
        .map(|t| (s(t), "freeze", 1.0))
        .into_iter()
        .collect();
    render_granular(config, &controls(mode), &events, &feed(mode, frames)).0
}

/// Compares the small buffer (15 wraps in [1 s, 4 s)) with the large one,
/// which has filled by 4 s and next wraps at 8 s. When `frozen`, both are
/// frozen once full (0.5 s and 4.5 s) and compared from 5.5 s.
fn compare(label: &str, mode: f32, frozen: bool) {
    let small = render_with(SMALL, mode, s(4.0), frozen.then_some(0.5));
    let large = render_with(LARGE, mode, s(7.0), frozen.then_some(4.5));
    let (small, large) = (&small[s(1.0)..], &large[s(5.5)..s(7.0)]);
    let (step, reference) = (max_step(small), max_step(large));
    assert!(reference > 1e-4, "{label}: reference output is silent");
    assert!(
        step <= 1.5 * reference,
        "{label}: largest step {step} with wraps vs {reference} without"
    );
    assert!(
        rms(small) >= 0.5 * rms(large),
        "{label}: level {} vs {} without wraps",
        rms(small),
        rms(large)
    );
    let (gap, ref_gap) = (
        longest_quiet_run(small, 1e-4),
        longest_quiet_run(large, 1e-4),
    );
    assert!(gap <= ref_gap + 48, "{label}: gap of {gap} samples");
}

#[test]
fn small_buffer_plays_seamlessly_across_wraps_in_every_mode() {
    for (name, mode) in MODES {
        compare(&format!("{name}, recording"), mode, false);
    }
}

/// The frozen buffer is read across its wrap point over and over.
/// Assumption: frozen `looping_delay` is left out because its loop length is
/// `size`, which the spec does not tie to the buffer's length.
#[test]
fn frozen_small_buffer_plays_seamlessly_across_wraps() {
    for (name, mode) in [("granular", GRANULAR), ("stretch", STRETCH)] {
        compare(&format!("{name}, frozen"), mode, true);
    }
}
