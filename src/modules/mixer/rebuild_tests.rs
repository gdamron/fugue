//! A mixer rebuilt by an edit carries on from what the mixer it replaces
//! was playing (FUG-375), so a reload that rebuilds it glides, not steps.

use super::tests::{glides, left_blocks, left_outputs, ramp_frames, BLOCK};
use super::*;

/// The left gain of a centred channel under the equal-power pan law.
fn centre() -> f32 {
    2.0_f32.sqrt().recip()
}

/// A replacement for `previous`, built with `next`'s controls, that has
/// taken over from it.
fn rebuilt(previous: &Mixer, next: Mixer) -> Mixer {
    let mut next = next;
    next.inherit(previous);
    next
}

#[test]
fn a_rebuild_glides_from_the_level_the_old_mixer_played() {
    let mut old = Mixer::new(1);
    let unity = left_outputs(&mut old, 1)[0];
    let mut new = rebuilt(&old, Mixer::new(1).with_level(0, 0.5));
    let outputs = left_outputs(&mut new, ramp_frames());
    assert!(glides(&outputs, unity, unity / 2.0), "{outputs:?}");
}

#[test]
fn a_rebuild_glides_from_the_master_the_old_mixer_played() {
    let mut old = Mixer::new(1).with_master(0.0);
    left_outputs(&mut old, 1);
    let mut new = rebuilt(&old, Mixer::new(1));
    let outputs = left_outputs(&mut new, ramp_frames());
    assert!(glides(&outputs, 0.0, centre()), "{outputs:?}");
}

#[test]
fn a_rebuild_mid_glide_carries_on_from_where_the_glide_was() {
    let mut old = Mixer::new(1);
    left_outputs(&mut old, 1);
    old.set_master(0.0);
    let turned_at = *left_outputs(&mut old, ramp_frames() / 2).last().unwrap();
    let mut new = rebuilt(&old, Mixer::new(1));
    let outputs = left_outputs(&mut new, ramp_frames());
    assert!(glides(&outputs, turned_at, centre()), "{outputs:?}");
}

#[test]
fn a_rebuild_glides_from_the_pan_the_old_mixer_played() {
    const HARD_LEFT: f32 = -1.0;
    let mut old = Mixer::new(1).with_pan(0, HARD_LEFT);
    let hard_left = left_outputs(&mut old, 1)[0];
    let mut new = rebuilt(&old, Mixer::new(1));
    let first = left_outputs(&mut new, 1)[0];
    // One frame into a glide back to the centre: still all but hard left.
    assert!(first > centre() && first <= hard_left, "{first}");
}

#[test]
fn a_channel_the_old_mixer_did_not_have_fades_in() {
    let mut old = Mixer::new(1);
    left_outputs(&mut old, 1);
    let mut new = rebuilt(&old, Mixer::new(2));
    let outputs = left_outputs_of_channel_1(&mut new, ramp_frames());
    assert!(glides(&outputs, 0.0, centre()), "{outputs:?}");
}

#[test]
fn a_mixer_that_never_played_hands_nothing_on() {
    let old = Mixer::new(1);
    let mut new = Mixer::new(1).with_master(0.25);
    assert!(!new.inherit(&old));
    let first = left_outputs(&mut new, 1)[0];
    assert_eq!(first, 0.25 * centre());
}

#[test]
fn another_type_of_module_hands_nothing_on() {
    let config = serde_json::json!({ "waveform": "sine" });
    let other = crate::modules::oscillator::OscillatorFactory
        .build(DEFAULT_SAMPLE_RATE, &config)
        .unwrap();
    let mut new = Mixer::new(1).with_master(0.25);
    assert!(!new.inherit(other.module.module()));
    let first = left_outputs(&mut new, 1)[0];
    assert_eq!(first, 0.25 * centre());
}

#[test]
fn taking_over_never_allocates() {
    let mut old = Mixer::new(2);
    left_blocks(&mut old, 1);
    let mut new = Mixer::new(4).with_master(0.5);
    let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
        new.inherit(&old);
        new.input_block_mut(0)[..BLOCK].fill(1.0);
        new.process(BLOCK);
    });
    assert_eq!((allocs, frees), (0, 0));
}

#[test]
fn rebuilds_in_one_block_lose_no_channel_a_later_one_restores() {
    let mut old = Mixer::new(2).with_level(0, 0.0);
    left_outputs_of_channel_1(&mut old, 1);
    // Two channels to one and back before the next block plays.
    let between = rebuilt(&old, Mixer::new(1));
    let mut new = rebuilt(&between, Mixer::new(2).with_level(0, 0.0));
    let first = left_outputs_of_channel_1(&mut new, 1)[0];
    assert_eq!(first, centre());
}

/// Plays `frames` single frames of a constant 1.0 on channel 1 and returns
/// the left output of each.
fn left_outputs_of_channel_1(mixer: &mut Mixer, frames: u32) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            mixer.set_input("audio.1", 1.0).unwrap();
            mixer.process(1);
            mixer.get_output("audio_left").unwrap()
        })
        .collect()
}
