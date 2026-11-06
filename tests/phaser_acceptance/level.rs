//! Criterion 3: at default settings the RMS of the mono fold (L+R)/2 is
//! within 3 dB of the mono input's RMS.
//!
//! Spec reading (conductor's clarification): "RMS of L+R" is the RMS of
//! (L+R)/2, the fold used for a mono output, against the RMS of the mono
//! source fed to `audio_left`. A steady sine is not used: a phaser's moving
//! notch legitimately takes a single partial's level up and down, and the
//! criterion is about programme material.

use super::rig::*;
use super::*;

/// Eight seconds (four sweep cycles at the default 0.5 Hz).
fn assert_level_within_three_db(name: &str, x: &[f32]) {
    let mut unit = build(json!({}));
    let (l, r) = unit.run_mono(x);
    let folded = mono_fold(&l, &r);
    assert_finite(&folded, name);
    let delta = db(rms(&folded) / rms(x));
    assert!(
        delta.abs() <= 3.0,
        "{name}: mono fold is {delta:.2} dB against the input"
    );
}

#[test]
fn white_noise_keeps_its_level_in_the_mono_fold() {
    for seed in [3, 17, 91] {
        assert_level_within_three_db("noise", &noise(secs(8.0), 0.5, seed));
    }
}

#[test]
fn a_pad_keeps_its_level_in_the_mono_fold() {
    for root in [55.0, 110.0, 220.0] {
        assert_level_within_three_db("pad", &pad(secs(8.0), root, 0.5));
    }
}

/// A quiet pad too: the soft clip must not be what keeps the level.
#[test]
fn a_quiet_pad_keeps_its_level_in_the_mono_fold() {
    assert_level_within_three_db("quiet pad", &pad(secs(8.0), 110.0, 0.05));
}
