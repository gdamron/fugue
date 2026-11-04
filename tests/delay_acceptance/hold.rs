//! Criterion 8: `hold` loops exactly one division, passes after the first
//! are bit-identical; entering and leaving hold do not click.

use super::rig::*;
use super::*;

/// Hold starts at frame S (a multiple of the engine's 64-frame block) on a
/// quarter note at 120 bpm (L = 24,000 samples).
const S: usize = 96_000;
const L: usize = 24_000;
const FRAMES: usize = S + 4 * L;

fn held_noise() -> (Vec<f32>, Vec<f32>) {
    let mut engine = engine(tap_doc(
        noise_source(0.7),
        "white",
        json!({ "division": "quarter_note", "bpm": 120.0 }),
    ));
    let first = render(&mut engine, S);
    set(&engine, "d", "hold", true);
    let second = render(&mut engine, FRAMES - S);
    let join = |a: Vec<f32>, b: Vec<f32>| [a, b].concat();
    (join(first.0, second.0), join(first.1, second.1))
}

/// Criterion 8: one pass later, every sample of the next pass is bit for bit
/// the same; the period is exactly one division, not a neighbouring length.
#[test]
fn hold_repeats_with_a_period_of_exactly_one_division() {
    let (_, out) = held_noise();
    // Pass 1 starts at S (a block of slack for when the write lands); passes
    // 2 and 3 are compared.
    let p2 = S + L + 256;
    let same = |lag: usize| (0..L).filter(|&i| out[p2 + i + lag] == out[p2 + i]).count();
    assert_eq!(same(L), L, "pass 3 is bit-identical to pass 2");
    for lag in [L - 1, L + 1, L / 2] {
        assert!(same(lag) < L / 100, "samples also repeat at lag {lag}");
    }
}

/// Criterion 8: what loops is the most recent division of the input. Away
/// from the 5 ms seam, a pass reproduces the dry signal from before hold
/// began, sample for sample (integer delay, inside the soft clip's linear
/// range): most 100-sample windows of a pass are found in that stretch.
#[test]
fn hold_loops_the_most_recent_division_of_input() {
    let (dry, out) = held_noise();
    let stretch = &dry[S - L - 1024..S + 1024];
    let p2 = S + L + 256;
    let found = (0..20)
        .filter(|w| {
            let at = p2 + w * 1_100 + 500;
            let window = &out[at..at + 100];
            (0..stretch.len() - 100).any(|j| stretch[j..j + 100] == *window)
        })
        .count();
    assert!(
        found >= 17,
        "only {found} of 20 windows of the loop come from the last division"
    );
}

/// Entering and leaving hold fade over 5 ms: a 437 Hz sine (a bad fit for the
/// loop length) shows no step beyond 1.5x the dry input's.
#[test]
fn entering_and_leaving_hold_do_not_click() {
    let mut doc = tap_doc(
        json!({ "id": "src", "type": "vca", "config": { "level": 0.5 } }),
        "audio",
        json!({ "division": "quarter_note", "bpm": 120.0 }),
    );
    doc["modules"].as_array_mut().unwrap().push(
        json!({ "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 437.0 } }),
    );
    doc["connections"]
        .as_array_mut()
        .unwrap()
        .push(conn("osc", "audio", "src", "audio"));
    let mut engine = engine(doc);
    let mut out = (Vec::new(), Vec::new());
    for (frames, hold) in [(S, false), (2 * L, true), (2 * L, false)] {
        set(&engine, "d", "hold", hold);
        let (l, r) = render(&mut engine, frames);
        out.0.extend(l);
        out.1.extend(r);
    }
    let (wet, dry) = (max_step(&out.1), max_step(&out.0));
    assert!(
        wet <= 1.5 * dry,
        "largest step {wet} exceeds 1.5 x dry {dry}"
    );
}
