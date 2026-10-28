//! A zone's level glides to a write while the zone sounds, as a mixer's
//! fader does, and moves at once while it is silent.

use super::{build_instrument, write_level_wav, TOL};
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::modules::mixer::gain_ramp_frames;

const RATE: u32 = 44_100;

/// One zone over every key, playing `asset` at `level`.
fn one_zone(asset: &std::path::Path, level: f32) -> serde_json::Value {
    serde_json::json!({
        "zones": [ {
            "root_note": 60, "key_range": [0, 127],
            "asset": asset.to_str().unwrap(), "level": level
        } ]
    })
}

/// The next `frames` frames of the left output, one block of one at a time.
fn heard(instrument: &mut Box<dyn crate::Module + Send>, frames: u32) -> Vec<f32> {
    (0..frames)
        .map(|_| {
            instrument.process(1);
            instrument.get_output("audio_left").unwrap()
        })
        .collect()
}

#[test]
fn a_level_written_while_a_note_sounds_glides_without_a_step() {
    let path = write_level_wav(0.5, 4 * RATE as usize / 100);
    let (mut instrument, _surface) = build_instrument(one_zone(&path, 1.0));
    instrument.set_control("note_on", 60.0).unwrap();
    let before = heard(&mut instrument, 1)[0];
    assert!((before - 0.5).abs() < TOL, "{before}");

    let glide = gain_ramp_frames(RATE);
    instrument.set_control("level.0", 0.0).unwrap();
    let mut outputs = Vec::with_capacity(glide as usize + 1);
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        for _ in 0..=glide {
            instrument.process(1);
            outputs.push(instrument.get_output("audio_left").unwrap());
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
fn a_level_written_while_the_zone_is_silent_holds_from_the_next_notes_first_frame() {
    let path = write_level_wav(0.5, 64);
    let (mut instrument, _surface) = build_instrument(one_zone(&path, 1.0));
    heard(&mut instrument, 4);
    instrument.set_control("level.0", 0.5).unwrap();
    instrument.set_control("note_on", 60.0).unwrap();
    let first = heard(&mut instrument, 1)[0];
    assert!((first - 0.25).abs() < TOL, "{first}");
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_note_ending_mid_glide_starts_its_next_note_at_the_level_written() {
    // Eight frames of sound: the note ends long before the 10 ms glide.
    let path = write_level_wav(0.5, 8);
    let (mut instrument, _surface) = build_instrument(one_zone(&path, 1.0));
    instrument.set_control("note_on", 60.0).unwrap();
    heard(&mut instrument, 2);
    instrument.set_control("level.0", 0.0).unwrap();
    heard(&mut instrument, 8);
    instrument.set_control("note_on", 60.0).unwrap();
    let first = heard(&mut instrument, 1)[0];
    assert_eq!(first, 0.0, "the muted zone stays muted");
    let _ = std::fs::remove_file(path);
}
