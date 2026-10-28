//! A play on a slot still sounding fades the sound it cuts off instead of
//! stopping it dead, and a swap during that fade leaves it on its buffer.

use super::tests::{build_kit, write_level_wav, write_levels_wav, TOL};
use super::voice::DECLICK_SECONDS;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::control_request::RtValue;
use crate::modules::sample_kit::controls::{index_of, KitControl};
use crate::modules::sample_loading::load_cached_sample;
use crate::payload::Payload;

const RATE: u32 = 44_100;

fn one_slot(asset: &std::path::Path) -> serde_json::Value {
    serde_json::json!({ "samples": [ { "key": 36, "asset": asset.to_str().unwrap() } ] })
}

fn play(kit: &mut Box<dyn crate::Module + Send>) {
    kit.apply(index_of(KitControl::Play, 1), RtValue::I32(0))
        .unwrap();
}

fn heard(kit: &mut Box<dyn crate::Module + Send>, frames: u32) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            kit.process(1);
            kit.get_output("audio_left").unwrap()
        })
        .collect()
}

/// The largest second difference over `frames`: a cut steps it to the
/// waveform's height, while a sine at 220 Hz bends it by under 0.0005.
fn worst_bend(frames: &[f32]) -> f32 {
    frames
        .windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).abs())
        .fold(0.0, f32::max)
}

fn declick_frames() -> u32 {
    (DECLICK_SECONDS * RATE as f32).ceil() as u32 + 1
}

#[test]
fn a_retrigger_mid_waveform_fades_the_old_sound_without_a_step() {
    // A 220 Hz sine from phase zero, as a struck sound starts.
    let sine: Vec<f32> = (0..RATE as usize)
        .map(|n| 0.5 * (std::f32::consts::TAU * 220.0 * n as f32 / RATE as f32).sin())
        .collect();
    let path = write_levels_wav(&sine);
    let (mut kit, _surface) = build_kit(one_slot(&path));
    play(&mut kit);
    // A quarter of a cycle in, where the sine is at its height.
    let mut frames = heard(&mut kit, 50);
    frames.reserve(declick_frames() as usize);
    play(&mut kit);
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        for _ in 0..declick_frames() {
            kit.process(1);
            frames.push(kit.get_output("audio_left").unwrap());
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    // The new play's onset bends the slope by its own first step (0.016
    // here) and the fade's start by its height times its slope (0.004):
    // a cut would step by the waveform's height, 0.5.
    let bend = worst_bend(&frames[40..]);
    assert!(bend < 0.03, "{bend}");
    // Past the fade the new play sounds alone, from its start.
    let alone = heard(&mut kit, 1)[0];
    let expected = sine[declick_frames() as usize];
    assert!((alone - expected).abs() < TOL, "{alone} vs {expected}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_retrigger_inside_the_fade_carries_the_first_fade_on() {
    let sine: Vec<f32> = (0..RATE as usize)
        .map(|n| 0.5 * (std::f32::consts::TAU * 220.0 * n as f32 / RATE as f32).sin())
        .collect();
    let path = write_levels_wav(&sine);
    let (mut kit, _surface) = build_kit(one_slot(&path));
    play(&mut kit);
    let mut frames = heard(&mut kit, 50);
    frames.reserve(2 * declick_frames() as usize + 1);
    play(&mut kit);
    frames.extend(heard(&mut kit, 1));
    // Played again a frame into the first fade, at the sine's height: had
    // the second fade replaced the first outright, the output would drop
    // by about 0.48 here.
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        play(&mut kit);
        for _ in 0..2 * declick_frames() {
            kit.process(1);
            frames.push(kit.get_output("audio_left").unwrap());
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    // A sine at 220 Hz moves by at most 0.016 a frame; the fades add their
    // slope (0.004 each).
    let jump = frames
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(jump < 0.03, "{jump}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_swap_during_the_fade_leaves_it_on_the_buffer_it_was_reading() {
    let kick = write_level_wav(0.5, RATE as usize);
    let quiet = write_level_wav(0.125, RATE as usize);
    let quieter = write_level_wav(0.0625, RATE as usize);
    let (mut kit, _surface) = build_kit(one_slot(&kick));
    let asset = index_of(KitControl::Asset(0), 1);
    let load = |path: &std::path::Path| {
        Payload::from_arc(load_cached_sample(path.to_str().unwrap(), RATE).unwrap())
    };
    play(&mut kit);
    heard(&mut kit, 8);
    // Swapped while it sounds: the kick rings out. A play then cuts the
    // ring-out, which fades while the quiet sample plays; a swap now
    // takes the place of the quiet one for the next play.
    let mut retired = Vec::with_capacity(2);
    retired.push(kit.apply_payload(asset, load(&quiet)).ok().unwrap());
    heard(&mut kit, 8);
    play(&mut kit);
    let first = heard(&mut kit, 1)[0];
    assert!(
        (first - 0.625).abs() < TOL,
        "kick fading over quiet: {first}"
    );
    let payload = load(&quieter);
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        retired.push(kit.apply_payload(asset, payload).ok().unwrap());
        kit.process(1);
    });
    assert_eq!((allocs, frees), (0, 0));
    let mut fade = vec![first, kit.get_output("audio_left").unwrap()];
    fade.extend(heard(&mut kit, declick_frames()));
    // The kick fades on evenly to the quiet sample alone.
    let step = 0.5 / (DECLICK_SECONDS * RATE as f32);
    for pair in fade.windows(2).take(declick_frames() as usize - 2) {
        assert!((pair[0] - pair[1] - step).abs() < 1e-3, "{fade:?}");
    }
    assert!((fade.last().unwrap() - 0.125).abs() < TOL, "{fade:?}");
    drop(retired);
    for path in [kick, quiet, quieter] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_retrigger_and_a_swap_at_one_instant_play_the_new_sample_over_the_fade() {
    let kick = write_level_wav(0.5, RATE as usize);
    let quiet = write_level_wav(0.125, RATE as usize);
    let (mut kit, _surface) = build_kit(one_slot(&kick));
    let asset = index_of(KitControl::Asset(0), 1);
    play(&mut kit);
    heard(&mut kit, 8);
    let payload = Payload::from_arc(load_cached_sample(quiet.to_str().unwrap(), RATE).unwrap());
    let mut retired = Vec::with_capacity(1);
    let mut fade = Vec::with_capacity(declick_frames() as usize);
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        // Played, then swapped, at one instant: the play takes the quiet
        // sample, while the kick it cut fades on its own buffer.
        play(&mut kit);
        retired.push(kit.apply_payload(asset, payload).ok().unwrap());
        for _ in 0..declick_frames() {
            kit.process(1);
            fade.push(kit.get_output("audio_left").unwrap());
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(
        (fade[0] - 0.625).abs() < TOL,
        "kick fading over quiet: {fade:?}"
    );
    let step = 0.5 / (DECLICK_SECONDS * RATE as f32);
    for pair in fade.windows(2).take(declick_frames() as usize - 2) {
        assert!((pair[0] - pair[1] - step).abs() < 1e-3, "{fade:?}");
    }
    assert!((fade.last().unwrap() - 0.125).abs() < TOL, "{fade:?}");
    drop(retired);
    for path in [kick, quiet] {
        let _ = std::fs::remove_file(path);
    }
}
