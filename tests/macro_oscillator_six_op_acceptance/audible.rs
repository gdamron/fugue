//! C4 (every patch audible) and C8 (triggered notes).

use super::rig::*;

/// Criterion C4: "Every patch is audible: droning at morph 0 and morph 0.5,
/// every patch's `audio` rms is above -50 dBFS at 220 Hz."
/// Assumption: rms over 0.5 s after 0.3 s of settling.
#[test]
fn every_patch_is_audible_while_droning() {
    let floor = 10f32.powf(-50.0 / 20.0);
    for morph in [0.0f32, 0.5] {
        for patch in 0..32 {
            let mut v = drone(patch, &[("morph", morph)]);
            let x = v.render(seconds(0.5));
            assert!(
                rms(&x) > floor,
                "{} (patch {patch}) at morph {morph}: rms {} is below -50 dBFS",
                PATCH_NAMES[patch],
                rms(&x)
            );
        }
    }
}

/// Criterion C8: "with `trigger` patched and held low, `audio` is silent (rms
/// below -80 dBFS) before the first trigger, and audible within 10 ms of a
/// rising edge, for every patch."
/// Assumptions: silence is measured from 20 ms after the start until the
/// edge; "audible within 10 ms" means a peak above 0.001 (-60 dBFS) in the
/// 10 ms after the edge (a slow-attack patch may need a looser reading); the
/// edge falls mid-block, 200 ms in.
#[test]
fn triggered_patches_are_silent_until_a_rising_edge_and_then_sound() {
    let silent = 10f32.powf(-80.0 / 20.0);
    let edge = seconds(0.2) + 11;
    for patch in 0..32 {
        let mut v = triggered(patch, &[]);
        let x = play(&mut v, &gate(seconds(0.6), &[(edge, seconds(0.5))]));
        let before = rms(&x[seconds(0.02)..edge]);
        assert!(
            before < silent,
            "{} (patch {patch}): rms {before} before the first trigger",
            PATCH_NAMES[patch]
        );
        let after = peak(&x[edge..edge + seconds(0.01)]);
        assert!(
            after > 0.001,
            "{} (patch {patch}): peak {after} in the 10 ms after the edge",
            PATCH_NAMES[patch]
        );
    }
}

/// Criterion C8 for short presses (a 5 ms trigger pulse plays through its
/// attack and then releases): the note is audible within 10 ms of its rising
/// edge for every patch, with the same threshold as above (peak above 0.001,
/// -60 dBFS).
#[test]
fn short_trigger_pulses_sound_within_ten_milliseconds() {
    let edge = seconds(0.2) + 11;
    for patch in 0..32 {
        let mut v = triggered(patch, &[]);
        let x = play(
            &mut v,
            &gate(seconds(0.5), &[(edge, edge + seconds(0.005))]),
        );
        let after = peak(&x[edge..edge + seconds(0.01)]);
        assert!(
            after > 0.001,
            "{} (patch {patch}): peak {after} in the 10 ms after a 5 ms pulse",
            PATCH_NAMES[patch]
        );
    }
}
