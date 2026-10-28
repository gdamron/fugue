//! A slot's level glides to a write while the slot sounds, as a mixer's
//! fader does, and moves at once while it is silent.

use super::tests::{build_kit, write_level_wav, TOL};
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::control_request::RtValue;
use crate::modules::mixer::gain_ramp_frames;
use crate::modules::sample_kit::controls::{index_of, KitControl};

const RATE: u32 = 44_100;

/// One slot keyed 36, playing `asset`.
fn one_slot(asset: &std::path::Path) -> serde_json::Value {
    serde_json::json!({ "samples": [ { "key": 36, "asset": asset.to_str().unwrap() } ] })
}

/// Plays slot 0, as a `play` write does.
fn play(kit: &mut Box<dyn crate::Module + Send>) {
    let play = index_of(KitControl::Play, 1);
    kit.apply(play, RtValue::I32(0)).unwrap();
}

/// The next `frames` frames of the left output, one block of one at a time.
fn heard(kit: &mut Box<dyn crate::Module + Send>, frames: u32) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            kit.process(1);
            kit.get_output("audio_left").unwrap()
        })
        .collect()
}

#[test]
fn a_level_written_while_a_slot_sounds_glides_without_a_step() {
    let path = write_level_wav(0.5, 4 * RATE as usize / 100);
    let (mut kit, _surface) = build_kit(one_slot(&path));
    play(&mut kit);
    let before = heard(&mut kit, 1)[0];
    assert!((before - 0.5).abs() < TOL, "{before}");

    let glide = gain_ramp_frames(RATE);
    kit.set_control("level.0", 0.0).unwrap();
    let mut outputs = Vec::with_capacity(glide as usize + 1);
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        for _ in 0..=glide {
            kit.process(1);
            outputs.push(kit.get_output("audio_left").unwrap());
        }
    });
    assert_eq!((allocs, frees), (0, 0));

    // Even steps over the glide, never the whole drop at once.
    let step = before / glide as f32;
    let mut previous = before;
    for (frame, &value) in outputs.iter().enumerate() {
        assert!(
            (previous - value - step).abs() < step * 0.01 || frame as u32 >= glide,
            "frame {frame}: {previous} -> {value}"
        );
        previous = value;
    }
    assert_eq!(outputs[glide as usize - 1], 0.0, "it lands on the level");
    assert_eq!(outputs[glide as usize], 0.0);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_level_written_while_the_slot_is_silent_holds_from_the_next_plays_first_frame() {
    let path = write_level_wav(0.5, 64);
    let (mut kit, _surface) = build_kit(one_slot(&path));
    heard(&mut kit, 4);
    kit.set_control("level.0", 0.5).unwrap();
    play(&mut kit);
    let first = heard(&mut kit, 1)[0];
    assert!((first - 0.25).abs() < TOL, "{first}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_slot_that_ends_mid_glide_starts_its_next_play_at_the_written_level() {
    let path = write_level_wav(0.5, 8);
    let (mut kit, _surface) = build_kit(one_slot(&path));
    play(&mut kit);
    heard(&mut kit, 2);
    kit.set_control("level.0", 0.5).unwrap();
    heard(&mut kit, 8);
    play(&mut kit);
    let first = heard(&mut kit, 1)[0];
    assert!((first - 0.25).abs() < TOL, "{first}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_play_while_the_slot_glides_starts_at_the_level_written() {
    let path = write_level_wav(0.5, RATE as usize);
    let (mut kit, _surface) = build_kit(one_slot(&path));
    play(&mut kit);
    heard(&mut kit, 4);
    kit.set_control("level.0", 0.5).unwrap();
    heard(&mut kit, 2);
    play(&mut kit);
    let first = heard(&mut kit, 1)[0];
    assert!((first - 0.25).abs() < TOL, "{first}");
    let _ = std::fs::remove_file(path);
}
