//! Criteria 2 and 4: tempo-synced delay times, and tempo changes re-timing
//! echoes without a click.

use super::rig::*;
use super::*;

/// Other note values, as beats x 60 / bpm: quarter at 90 bpm is 32,000
/// samples; quarter triplet (2/3 beat) at 120 bpm is 16,000.
#[test]
fn other_divisions_follow_the_tempo() {
    let echo = |config: Value| {
        let mut d = delay(config);
        d.render(&impulses(40_000, &[(0, 0.5)]))
    };
    assert_hits(
        &echo(json!({ "division": "quarter_note", "bpm": 90.0 })),
        &[(32_000, 0.5)],
    );
    assert_hits(
        &echo(json!({ "division": "quarter_triplet", "bpm": 120.0 })),
        &[(16_000, 0.5)],
    );
    assert_hits(
        &echo(json!({ "division": "off", "time": 0.25, "bpm": 90.0 })),
        &[(12_000, 0.5)],
    );
}

/// Impulses at A, C and D, with the tempo going 120 -> 240 bpm (quarter note:
/// 24,000 -> 12,000 samples) at block start S:
/// - A is echoed before S at the old time;
/// - C's pending echo moves from S + 18,000 to S + 6,000;
/// - D's echo lands 10 samples after the first block plus the 20 ms crossfade
///   that follow S, so the new time is in force within one block.
fn retime(setup: fn(&mut Delay), change: fn(&mut Delay)) {
    const S: usize = 400 * BLOCK;
    let (a, c, dd) = (1_000, S - 6_000, S + BLOCK + 970 - 12_000);
    let mut d = delay(json!({ "division": "quarter_note", "bpm": 120.0 }));
    setup(&mut d);
    let input = impulses(90_000, &[(a, 0.5), (c, 0.5), (dd, 0.5)]);
    let out = d.run(&input, BLOCK, |frame, d| {
        if frame == S {
            change(d);
        }
    });
    assert_hits(
        &out,
        &[(a + 24_000, 0.5), (S + BLOCK + 970, 0.5), (c + 12_000, 0.5)],
    );
}

fn tempo_input(d: &mut Delay) {
    d.connect_bpm(120.0);
}

/// Criterion 4 (timing): a change on the `bpm` control re-times echoes.
#[test]
fn bpm_control_change_retimes_echoes_within_one_block() {
    retime(|_| {}, |d| d.set("bpm", 240.0));
}

/// Criterion 4 (timing): a change on the `bpm` input re-times echoes.
#[test]
fn bpm_input_change_retimes_echoes_within_one_block() {
    retime(tempo_input, |d| d.bpm_in = Some(240.0));
}

/// Criterion 4 (no click): a sine at 450 Hz comes back anti-phase after a
/// 0.25 s change in delay time, the worst case for a crossfade. The largest
/// step of the output stays within 1.5x the dry input's.
fn no_click(setup: fn(&mut Delay), change: fn(&mut Delay)) {
    let input = sine(150_000, 450.0, 0.5);
    let mut d = delay(json!({ "division": "quarter_note", "bpm": 120.0 }));
    setup(&mut d);
    let out = d.run(&input, BLOCK, |frame, d| {
        if frame == 400 * BLOCK {
            change(d);
        }
    });
    assert!(peak(&out) > 0.4, "the sine is heard");
    let (wet, dry) = (max_step(&out), max_step(&input));
    assert!(
        wet <= 1.5 * dry,
        "largest step {wet} exceeds 1.5 x dry {dry}"
    );
}

#[test]
fn bpm_control_change_does_not_click() {
    no_click(|_| {}, |d| d.set("bpm", 240.0));
}

#[test]
fn bpm_input_change_does_not_click() {
    no_click(tempo_input, |d| d.bpm_in = Some(240.0));
}
