//! A zone's audio is swapped in without the audio thread freeing the audio
//! it replaces, or the samples its voices let go of.

use super::{build_instrument, write_level_wav, Surface, TOL};
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::control_request::ControlIndex;
use crate::modules::sample_instrument::controls::{index_of, InstrumentControl};
use crate::payload::Payload;

/// Zone 0's next audio, loaded from `asset` as a control thread loads it.
fn loaded(surface: &Surface, asset: &std::path::Path) -> Payload {
    let value = asset.to_str().unwrap().into();
    let (payload, _) = surface.prepare_payload("asset.0", &value).unwrap().unwrap();
    payload
}

const ASSET: ControlIndex = index_of(InstrumentControl::Asset(0), 1);

#[test]
fn applying_zone_audio_frees_nothing_on_the_audio_thread() {
    let first = write_level_wav(0.5, 44_100);
    let second = write_level_wav(0.25, 44_100);
    let config = serde_json::json!({
        "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": first.to_str().unwrap() } ]
    });
    let (mut instrument, surface) = build_instrument(config);
    instrument.set_control("note_on", 60.0).unwrap();
    instrument.process(1);

    let payload = loaded(&surface, &second);
    let (retired, allocs, frees) = allocator_events(|| {
        // Dropping a payload or what it retires here panics in a debug
        // build.
        let _audio = AudioThreadScope::enter();
        let retired = instrument.apply_payload(ASSET, payload).ok().unwrap();
        instrument.process(1);
        retired
    });
    assert_eq!((allocs, frees), (0, 0));
    // The sounding voice finishes on the old audio; the next note starts
    // on the new.
    assert!((instrument.get_output("audio_left").unwrap() - 0.5).abs() < TOL);
    drop(retired);
    instrument.set_control("note_on", 64.0).unwrap();
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.75).abs() < 2.0 * TOL);

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}

#[test]
fn a_voice_letting_go_of_twice_replaced_audio_frees_nothing() {
    let first = write_level_wav(0.5, 44_100);
    let second = write_level_wav(0.25, 44_100);
    let third = write_level_wav(0.125, 44_100);
    let config = serde_json::json!({
        "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": first.to_str().unwrap() } ]
    });
    let (mut instrument, surface) = build_instrument(config);
    instrument.set_control("note_on", 60.0).unwrap();
    instrument.process(1);

    // The sounding voice keeps the first audio through two swaps, each
    // retiring the zone audio it replaces, freed here on a control thread.
    for next in [&second, &third] {
        let payload = loaded(&surface, next);
        drop(instrument.apply_payload(ASSET, payload).ok().unwrap());
        instrument.process(1);
    }

    // Retriggering the note moves its voice to the third audio, fading out
    // the first and then letting go of it: the cache still owns it.
    instrument.set_control("note_on", 60.0).unwrap();
    let (_, allocs, frees) = allocator_events(|| {
        for _ in 0..4_410 {
            instrument.process(1);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!((instrument.get_output("audio_left").unwrap() - 0.125).abs() < TOL);

    for path in [first, second, third] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_note_taking_audio_swapped_in_at_its_instant_frees_nothing() {
    let first = write_level_wav(0.5, 44_100);
    let second = write_level_wav(0.25, 44_100);
    let config = serde_json::json!({
        "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": first.to_str().unwrap() } ]
    });
    let (mut instrument, surface) = build_instrument(config);
    instrument.set_control("note_on", 60.0).unwrap();

    // The note has sounded nothing yet, so it lets go of the first audio
    // and takes the second.
    let payload = loaded(&surface, &second);
    let (retired, allocs, frees) = allocator_events(|| {
        let _audio = AudioThreadScope::enter();
        let retired = instrument.apply_payload(ASSET, payload).ok().unwrap();
        instrument.process(1);
        retired
    });
    assert_eq!((allocs, frees), (0, 0));
    drop(retired);
    assert!((instrument.get_output("audio_left").unwrap() - 0.25).abs() < TOL);

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}
