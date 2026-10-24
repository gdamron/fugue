//! A sample kit on a live graph: `play` names a slot by key or name and
//! every play is applied, `level.N` scales its slot, `asset.N` swaps its
//! sample without allocating on the audio thread, `key.N` is fixed, and a
//! block renders while another thread holds everything a write to the kit
//! takes.

use std::thread;

use super::migrated::{block_while_held, read};
use super::requests::outcomes;
use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::control_request::Outcome;
use crate::invention::declared::DeclaredSurface;
use crate::modules::sample_loading::hold_sample_cache;
use crate::ControlValue;

const DAC: &str = r#"{
    "version": "1.0.0",
    "modules": [{ "id": "dac", "type": "dac" }],
    "connections": []
}"#;

/// Writes a mono WAV at the engine rate, every frame at `level`, into
/// `dir` as `name`.
fn level_wav(dir: &std::path::Path, name: &str, level: f32, frames: usize) -> String {
    let path = dir.join(name);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for _ in 0..frames {
        writer
            .write_sample((level * i16::MAX as f32) as i16)
            .unwrap();
    }
    writer.finalize().unwrap();
    path.to_string_lossy().into_owned()
}

/// A kit `kit` of `samples` (each a key and an asset), live.
fn rig(samples: serde_json::Value) -> Rig {
    let mut rig = Rig::new(DAC);
    let kit = rig.build(
        "kit",
        "sample_kit",
        serde_json::json!({ "samples": samples }),
    );
    rig.live
        .edit(|change| {
            change.upsert("kit", kit);
            Ok(())
        })
        .unwrap();
    rig.render(1);
    rig.live.reclaim();
    rig
}

/// A kick (key 36, at 0.5) and a ride (named, at 0.25), each 1 s long.
fn kick_and_ride(dir: &std::path::Path) -> Rig {
    let kick = level_wav(dir, "kick.wav", 0.5, SAMPLE_RATE as usize);
    let ride = level_wav(dir, "ride.wav", 0.25, SAMPLE_RATE as usize);
    rig(serde_json::json!([
        { "key": 36, "asset": kick },
        { "key": "ride", "asset": ride }
    ]))
}

fn set(rig: &Rig, key: &str, value: ControlValue) -> Result<(), String> {
    let surface = rig.live.control_surfaces.lock().unwrap()["kit"].clone();
    surface.set_control(key, value)
}

/// The kit's left output at the first frame of the block just rendered.
fn heard(rig: &Rig) -> f32 {
    rig.graph.modules["kit"].module().output_block(0)[0]
}

/// One block rendered inside an audio-thread scope (where a payload
/// dropping panics in debug builds), counting its allocations and frees.
fn audio_block(rig: &mut Rig) -> (usize, usize) {
    let graph = &mut rig.graph;
    thread::scope(|scope| {
        scope
            .spawn(|| {
                let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
                let ((), allocs, frees) = allocator_events(|| {
                    let _audio = AudioThreadScope::enter();
                    graph.process_block(&mut left, &mut right);
                });
                (allocs, frees)
            })
            .join()
            .unwrap()
    })
}

fn all_applied(rig: &mut Rig, count: usize) {
    let outcomes = outcomes(rig);
    assert_eq!(outcomes.len(), count, "{outcomes:?}");
    for (_, outcome) in outcomes {
        assert!(matches!(outcome, Outcome::Applied { .. }), "{outcome:?}");
    }
}

/// 16-bit quantization tolerance.
const TOL: f32 = 2e-3;

#[test]
fn a_block_renders_while_a_control_thread_holds_the_kits_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = kick_and_ride(dir.path());
    let quiet = level_wav(dir.path(), "quiet.wav", 0.125, SAMPLE_RATE as usize);
    set(&rig, "asset.0", quiet.as_str().into()).unwrap();
    set(&rig, "level.0", 0.5.into()).unwrap();
    set(&rig, "play", 36.0.into()).unwrap();

    let surface = rig.live.control_surfaces.lock().unwrap()["kit"].clone();
    let kit = (surface.as_any())
        .and_then(|any| any.downcast_ref::<DeclaredSurface>())
        .expect("the kit's controls are declared");
    let held = block_while_held(&mut rig, || (kit.hold_state(), hold_sample_cache()));
    assert_eq!(held, Ok((0, 0)));
    assert!((heard(&rig) - 0.0625).abs() < TOL, "{}", heard(&rig));
    all_applied(&mut rig, 3);
}

#[test]
fn play_by_slot_name_and_by_slot_number_both_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = kick_and_ride(dir.path());

    set(&rig, "play", "ride".into()).unwrap();
    rig.render(1);
    assert!((heard(&rig) - 0.25).abs() < TOL);
    set(&rig, "play", 36.0.into()).unwrap();
    rig.render(1);
    assert!((heard(&rig) - 0.75).abs() < TOL, "the kick joins the ride");
    set(&rig, "play", "36".into()).unwrap();
    rig.render(1);
    assert!((heard(&rig) - 0.75).abs() < TOL);
    all_applied(&mut rig, 3);

    for (value, reason) in [
        (ControlValue::from("crash"), "No sample slot named 'crash'"),
        (ControlValue::Number(40.0), "No sample slot with key 40"),
        (ControlValue::Number(f32::NAN), "Invalid slot key"),
        (ControlValue::Bool(true), "not a boolean"),
    ] {
        let error = set(&rig, "play", value.clone()).unwrap_err();
        assert!(error.contains(reason), "{value:?}: {error}");
    }
    rig.render(1);
    assert!(
        outcomes(&mut rig).is_empty(),
        "a refused play sends nothing"
    );
}

#[test]
fn rapid_retriggers_on_one_slot_are_all_applied() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = kick_and_ride(dir.path());
    for _ in 0..4 {
        set(&rig, "play", 36.0.into()).unwrap();
    }
    set(&rig, "play", "ride".into()).unwrap();
    rig.render(1);
    assert!((heard(&rig) - 0.75).abs() < TOL);
    all_applied(&mut rig, 5);
}

#[test]
fn a_level_write_scales_its_slot_clamped_to_twice_unity() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = kick_and_ride(dir.path());
    set(&rig, "level.0", 3.0.into()).unwrap();
    set(&rig, "play", 36.0.into()).unwrap();
    rig.render(1);
    assert_eq!(read(&rig, "kit", "level.0"), ControlValue::Number(2.0));
    assert!((heard(&rig) - 1.0).abs() < 2.0 * TOL);
    assert_eq!(read(&rig, "kit", "level.1"), ControlValue::Number(1.0));
}

#[test]
fn an_asset_swap_waits_for_the_next_play_and_frees_nothing_on_the_audio_thread() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = kick_and_ride(dir.path());
    set(&rig, "play", 36.0.into()).unwrap();
    rig.render(1);
    assert!((heard(&rig) - 0.5).abs() < TOL);

    let quiet = level_wav(dir.path(), "quiet.wav", 0.125, SAMPLE_RATE as usize);
    set(&rig, "asset.0", quiet.as_str().into()).unwrap();
    assert_eq!(read(&rig, "kit", "asset.0"), quiet.as_str().into());
    assert_eq!(audio_block(&mut rig), (0, 0));
    assert_eq!(heard(&rig), 0.0, "the old sample stops, the new one waits");

    set(&rig, "play", 36.0.into()).unwrap();
    assert_eq!(audio_block(&mut rig), (0, 0));
    assert!((heard(&rig) - 0.125).abs() < TOL);
    rig.live.reclaim();
}

#[test]
fn slot_keys_are_fixed_and_a_slot_past_the_count_is_no_control() {
    let dir = tempfile::tempdir().unwrap();
    let rig = kick_and_ride(dir.path());
    assert_eq!(read(&rig, "kit", "key.0"), "36".into());
    assert_eq!(read(&rig, "kit", "key.1"), "ride".into());
    let error = set(&rig, "key.0", "38".into()).unwrap_err();
    assert!(error.contains("fixed at build time"), "{error}");

    for key in ["asset.2", "level.2", "key.2"] {
        assert!(set(&rig, key, 1.0.into()).is_err(), "{key}");
        let surfaces = rig.live.control_surfaces.lock().unwrap();
        assert!(surfaces["kit"].get_control(key).is_err(), "{key}");
    }
    let surfaces = rig.live.control_surfaces.lock().unwrap();
    let keys: Vec<String> = (surfaces["kit"].controls().into_iter())
        .map(|meta| meta.key)
        .collect();
    assert_eq!(
        keys,
        ["play", "asset.0", "asset.1", "level.0", "level.1", "key.0", "key.1"]
    );
}
