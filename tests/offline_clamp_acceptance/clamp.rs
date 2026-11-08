//! Criterion: an invention that exceeds 1.0 renders offline with every
//! sample in [-1, 1].

use super::support::*;

fn assert_in_range(out: &[f32], what: &str) {
    for (i, x) in out.iter().enumerate() {
        assert!(x.is_finite(), "{what}: sample {i} is {x}");
        assert!((-1.0..=1.0).contains(x), "{what}: sample {i} is {x}");
    }
}

#[test]
fn a_loud_sine_renders_inside_unit_range() {
    // +12 dB on a full-scale sine peaks near 3.98; the dac's soft clip is off.
    let out = offline(&tone("sine", 440.0, 12.0), 9_600, 512);
    assert_in_range(&out, "sine at +12 dB");
    let high = out.iter().cloned().fold(f32::MIN, f32::max);
    let low = out.iter().cloned().fold(f32::MAX, f32::min);
    assert_eq!(high, MAX_BELOW_ONE, "the positive peaks sit at the clamp");
    assert_eq!(low, -1.0, "the negative peaks sit at the clamp");
}

#[test]
fn every_channel_and_waveform_is_limited() {
    for (waveform, db) in [
        ("sine", 24.0),
        ("square", 6.0),
        ("sawtooth", 12.0),
        ("triangle", 12.0),
    ] {
        let out = offline(&tone(waveform, 220.0, db), 4_800, 480);
        assert_in_range(&out, waveform);
        for side in 0..2 {
            let loud = out
                .iter()
                .skip(side)
                .step_by(2)
                .filter(|x| x.abs() >= 0.99)
                .count();
            assert!(
                loud > 100,
                "{waveform} channel {side} is driven into the clamp"
            );
        }
    }
}

#[test]
fn an_over_level_square_is_written_at_the_clamp_values() {
    // A square at +12 dB is far over level on both halves: positive samples
    // render as the largest value below 1.0 (as the live device writes them),
    // negative ones as -1.0.
    let out = offline(&tone("square", 100.0, 12.0), 4_800, 4_800);
    let mut positive = 0;
    let mut negative = 0;
    for x in &out {
        match *x {
            v if v == MAX_BELOW_ONE => positive += 1,
            v if v == -1.0 => negative += 1,
            v => assert!(v.abs() < 0.5, "a transition sample, got {v}"),
        }
    }
    assert!(
        positive > 1_000 && negative > 1_000,
        "{positive} {negative}"
    );
}

#[test]
fn the_clamp_holds_for_any_buffer_size() {
    let json = tone("sine", 333.0, 18.0);
    let whole = offline(&json, 6_000, 6_000);
    assert_in_range(&whole, "one buffer");
    for chunk in [1, 7, 64, 100, 513, 5_999] {
        assert_eq!(
            hash(&offline(&json, 6_000, chunk)),
            hash(&whole),
            "buffer size {chunk}"
        );
    }
}
