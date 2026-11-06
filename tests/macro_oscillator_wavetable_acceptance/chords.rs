//! Part B: the wavetable half of the `chords` model (`morph` above 0.5).

use super::bounded::stress;
use super::morph::edge_and_other_steps;
use super::pitch::{assert_pitch, MEASURED};
use super::rig::*;

/// Criterion B1: "At `morph` 1, `chords` is audible and its `aux` (the chord
/// root alone) is within +-2 cents of the played pitch at 110 and 440 Hz
/// (timbre 0.5)."
/// Assumption: audible means `audio` peak above 0.01 and RMS above 0.003;
/// other macros stay at their defaults.
#[test]
fn chords_at_morph_one_is_audible_and_its_root_is_in_tune() {
    let position = chords();
    for hz in [110.0f32, 440.0] {
        let mut v = settled(
            position,
            &[("frequency", hz), ("morph", 1.0), ("timbre", 0.5)],
        );
        let (audio, aux) = v.render_both(MEASURED);
        assert!(peak(&audio) > 0.01, "chords audio silent at {hz} Hz");
        assert!(rms(&audio) > 0.003, "chords audio too quiet at {hz} Hz");
        assert_pitch("chords aux at morph 1", &aux, f64::from(hz));
    }
}

/// Criterion B2: "A `morph` sweep from 0 to 1 over 200 ms (written once per
/// 64-frame block, 220 Hz) is click-free: its largest sample-to-sample step is
/// no more than 1.5x the larger of the steady-state largest steps at morph 0
/// and at morph 1."
/// Assumptions: steady-state steps are measured over 1 s after 0.3 s of
/// settling; the reverse sweep (1 to 0) is held to the same bound.
#[test]
fn chords_morph_sweep_is_click_free() {
    let position = chords();
    let steady = |morph: f32| {
        let mut v = settled(position, &[("frequency", 220.0), ("morph", morph)]);
        max_step(&v.render(seconds(1.0)))
    };
    let bound = 1.5 * steady(0.0).max(steady(1.0));
    for (from, to) in [(0.0f32, 1.0f32), (1.0, 0.0)] {
        let mut v = settled(position, &[("frequency", 220.0), ("morph", from)]);
        let x = v.sweep("morph", from, to, seconds(0.2));
        assert!(peak(&x) > 0.01, "sweep is silent");
        let step = max_step(&x);
        assert!(
            step <= bound,
            "morph {from} -> {to}: largest step {step} exceeds 1.5x steady state ({bound})"
        );
        let (edge, other) = edge_and_other_steps(&x);
        assert!(
            edge <= 1.5 * other.max(bound / 1.5),
            "morph {from} -> {to}: block-edge step {edge} stands out (elsewhere {other})"
        );
    }
}

/// Criterion B3: "The upper half differs in sound from the lower: the spectra
/// of `audio` at morph 0.25 and morph 1 differ clearly (for example the ratio
/// of power above 2 kHz to total power differs by more than 3 dB)."
/// Assumption: the example measure is used as is, at 220 Hz with the other
/// macros at their defaults, over a 16384-frame Blackman-Harris window.
#[test]
fn chords_upper_half_sounds_different_from_the_lower_half() {
    let position = chords();
    let share = |morph: f32| {
        let mut v = settled(position, &[("frequency", 220.0), ("morph", morph)]);
        high_share_db(
            &power_spectrum(&v.render(1 << 14), 1 << 14),
            1 << 14,
            2000.0,
        )
    };
    let (lower, upper) = (share(0.25), share(1.0));
    assert!(
        (lower - upper).abs() > 3.0,
        "power above 2 kHz is {lower:.1} dB of total at morph 0.25 and {upper:.1} dB at morph 1"
    );
}

/// Part B bounded (project definition of done): `chords` with `morph` over
/// its upper half and every other control and input modulated at random stays
/// finite and within +-1.
#[test]
fn chords_wavetable_half_stays_bounded_under_random_modulation() {
    stress(0xC0DE5, 3.0, chords(), &[("morph", 0.5, 1.0)]);
}
