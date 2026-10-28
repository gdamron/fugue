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

#[test]
fn a_burst_of_stolen_zones_never_steps_a_sounding_zones_level() {
    use crate::control_request::RtValue;
    use crate::modules::sample_instrument::controls::{index_of, InstrumentControl as C};
    // Zone 0 holds note 60; zones 1..=32 each own one key above it and
    // play silence, so only zone 0 is heard.
    const ZONES: usize = 33;
    let path = write_level_wav(0.5, RATE as usize);
    let silent = write_level_wav(0.0, RATE as usize);
    let zones: Vec<_> = (0..ZONES)
        .map(|zone| {
            let key = 60 + zone;
            let asset = if zone == 0 { &path } else { &silent };
            let asset = asset.to_str().unwrap();
            serde_json::json!({ "root_note": key, "key_range": [key, key], "asset": asset })
        })
        .collect();
    let config = serde_json::json!({ "voices": 2, "zones": zones });
    let (mut instrument, _surface) = build_instrument(config);
    let mut apply = |control, value: f32| {
        instrument
            .apply(index_of(control, ZONES), RtValue::F32(value))
            .unwrap();
    };
    apply(C::NoteOn, 60.0);
    // In the order written, between two renders: each zone sounds, glides
    // and is stolen in turn, while zone 0 sounds on.
    for zone in 1..ZONES {
        let key = 60.0 + zone as f32;
        apply(C::NoteOn, key);
        apply(C::Level(zone), 0.0);
        apply(C::NoteOff, key);
    }
    apply(C::Level(0), 0.0);
    let ((), allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        instrument.process(1);
    });
    assert_eq!((allocs, frees), (0, 0));
    let level = instrument.get_output("audio_left").unwrap();
    assert!(level > 0.4, "zone 0 glides rather than stepping: {level}");
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(silent);
}
