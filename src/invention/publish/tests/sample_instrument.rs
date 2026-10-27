//! A sample instrument on a live graph: notes apply in the order written,
//! however many are sent, a scheduled chord plays whole, `asset.N` swaps a
//! zone's sample under playback without the audio thread allocating or
//! freeing, and a block renders while another thread holds everything a
//! write to the instrument takes.

use std::thread;

use super::migrated::{block_while_held, read, schedule};
use super::requests::{counted_block, outcomes};
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

/// Writes a mono WAV at the engine rate, one second of `level`, into `dir`
/// as `name`. Any pitch plays it back at `level`.
fn level_wav(dir: &std::path::Path, name: &str, level: f32) -> String {
    let path = dir.join(name);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for _ in 0..SAMPLE_RATE {
        let sample = (level * i16::MAX as f32) as i16;
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
    path.to_string_lossy().into_owned()
}

/// An instrument `inst` with one zone over every key, rooted at 60,
/// playing a sample at 0.5, and a release short enough that a note
/// released at a block's start is silent by its end. Live.
fn rig(dir: &std::path::Path) -> Rig {
    let mut rig = Rig::new(DAC);
    let inst = rig.build("inst", "sample_instrument", config(dir));
    upsert(&rig, inst);
    rig.render(1);
    rig.live.reclaim();
    rig
}

fn config(dir: &std::path::Path) -> serde_json::Value {
    let asset = level_wav(dir, "half.wav", 0.5);
    serde_json::json!({
        "release": 0.001,
        "zones": [{ "root_note": 60, "key_range": [0, 127], "asset": asset }]
    })
}

fn upsert(rig: &Rig, inst: change::BuiltModule) {
    let edit = |change: &mut GraphChange| {
        change.upsert("inst", inst);
        Ok(())
    };
    rig.live.edit(edit).unwrap();
}

fn set(rig: &Rig, key: &str, value: ControlValue) -> Result<(), String> {
    let surface = rig.live.control_surfaces.lock().unwrap()["inst"].clone();
    surface.set_control(key, value)
}

/// The instrument's left output at the last frame of the block just
/// rendered.
fn heard(rig: &Rig) -> f32 {
    rig.graph.modules["inst"].module().output_block(0)[63]
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
fn a_block_renders_while_a_control_thread_holds_the_instruments_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig(dir.path());
    let quiet = level_wav(dir.path(), "quiet.wav", 0.25);
    set(&rig, "asset.0", quiet.as_str().into()).unwrap();
    set(&rig, "level.0", 0.5.into()).unwrap();
    set(&rig, "note_on", 60.0.into()).unwrap();

    let surface = rig.live.control_surfaces.lock().unwrap()["inst"].clone();
    let inst = (surface.as_any())
        .and_then(|any| any.downcast_ref::<DeclaredSurface>())
        .expect("the instrument's controls are declared");
    let held = block_while_held(&mut rig, || (inst.hold_state(), hold_sample_cache()));
    assert_eq!(held, Ok((0, 0)));
    assert!((heard(&rig) - 0.125).abs() < TOL, "{}", heard(&rig));
    all_applied(&mut rig, 3);
}

#[test]
fn notes_apply_in_the_order_written() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig(dir.path());
    set(&rig, "note_on", 60.0.into()).unwrap();
    set(&rig, "note_on", 64.0.into()).unwrap();
    set(&rig, "note_off", 60.0.into()).unwrap();
    rig.render(1);
    assert!((heard(&rig) - 0.5).abs() < TOL, "64 alone: {}", heard(&rig));
    all_applied(&mut rig, 3);
}

#[test]
fn a_note_off_after_hundreds_of_note_ons_still_applies() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig(dir.path());
    for _ in 0..250 {
        set(&rig, "note_on", 60.0.into()).unwrap();
    }
    set(&rig, "note_off", 60.0.into()).unwrap();
    // Past the retriggers' declick fades.
    rig.render(10);
    assert!(heard(&rig).abs() < TOL, "released: {}", heard(&rig));
    all_applied(&mut rig, 251);
}

#[test]
fn numeric_string_notes_are_accepted_live() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig(dir.path());
    set(&rig, "note_on", "60".into()).unwrap();
    rig.render(1);
    assert!((heard(&rig) - 0.5).abs() < TOL);
    set(&rig, "note_off", "60".into()).unwrap();
    rig.render(1);
    assert!(heard(&rig).abs() < TOL);
    all_applied(&mut rig, 2);
}

#[test]
fn a_note_written_before_the_instrument_runs_plays_as_it_binds() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = Rig::new(DAC);
    let inst = rig.build("inst", "sample_instrument", config(dir.path()));
    let built = inst.surface.clone().unwrap();
    built.set_control("note_on", 64.0.into()).unwrap();
    upsert(&rig, inst);
    rig.render(1);
    assert!((heard(&rig) - 0.5).abs() < TOL, "{}", heard(&rig));
    rig.render(1);
    assert!((heard(&rig) - 0.5).abs() < TOL, "it fired once, and holds");
}

#[test]
fn a_zone_swap_under_playback_allocates_and_frees_nothing_on_the_audio_thread() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig(dir.path());
    let quiet = level_wav(dir.path(), "quiet.wav", 0.25);
    let quieter = level_wav(dir.path(), "quieter.wav", 0.125);
    set(&rig, "note_on", 60.0.into()).unwrap();
    rig.render(1);

    // The sounding note finishes on the sample it started on.
    for asset in [&quiet, &quieter] {
        set(&rig, "asset.0", asset.as_str().into()).unwrap();
        assert_eq!(read(&rig, "inst", "asset.0"), asset.as_str().into());
        assert_eq!(audio_block(&mut rig), (0, 0));
        assert!((heard(&rig) - 0.5).abs() < TOL, "the note rings on");
    }
    rig.live.reclaim();

    // The next note takes the new one; a retrigger moves the sounding
    // note's voice onto it, letting go of the old sample as it fades.
    set(&rig, "note_on", 64.0.into()).unwrap();
    assert_eq!(audio_block(&mut rig), (0, 0));
    assert!((heard(&rig) - 0.625).abs() < TOL, "{}", heard(&rig));
    set(&rig, "note_on", 60.0.into()).unwrap();
    for _ in 0..10 {
        assert_eq!(audio_block(&mut rig), (0, 0));
    }
    assert!((heard(&rig) - 0.25).abs() < TOL, "{}", heard(&rig));
}

#[test]
fn a_scheduled_chord_plays_whole_on_its_step_and_releases_on_the_next() {
    let dir = tempfile::tempdir().unwrap();
    let mut rig = rig(dir.path());
    let entry = |step: u32, control: &str, note: f32| {
        serde_json::json!({ "at_step": step, "module": "inst", "control": control, "value": note })
    };
    let chord = [60.0, 64.0, 67.0];
    let entries: Vec<_> = (chord.iter().map(|&note| entry(0, "note_on", note)))
        .chain(chord.iter().map(|&note| entry(1, "note_off", note)))
        .collect();
    schedule(&mut rig, serde_json::json!({ "schedule": entries }));

    rig.live.write_input("sched", "clock", 1.0).unwrap();
    for _ in 0..2 {
        assert_eq!(counted_block(&mut rig), (0, 0));
    }
    assert!((heard(&rig) - 1.5).abs() < 2.0 * TOL, "{}", heard(&rig));
    rig.live.write_input("sched", "clock", 0.0).unwrap();
    assert_eq!(counted_block(&mut rig), (0, 0));
    rig.live.write_input("sched", "clock", 1.0).unwrap();
    for _ in 0..2 {
        assert_eq!(counted_block(&mut rig), (0, 0));
    }
    assert!(heard(&rig).abs() < TOL, "{}", heard(&rig));
}

#[test]
fn root_notes_are_read_only_and_a_zone_past_the_count_is_no_control() {
    let dir = tempfile::tempdir().unwrap();
    let rig = rig(dir.path());
    assert_eq!(read(&rig, "inst", "root_note.0"), ControlValue::Number(60.0));
    let error = set(&rig, "root_note.0", 62.0.into()).unwrap_err();
    assert!(error.contains("read-only"), "{error}");
    assert_eq!(read(&rig, "inst", "root_note.0"), ControlValue::Number(60.0));

    for key in ["asset.1", "level.1", "root_note.1"] {
        assert!(set(&rig, key, 1.0.into()).is_err(), "{key}");
        let surfaces = rig.live.control_surfaces.lock().unwrap();
        assert!(surfaces["inst"].get_control(key).is_err(), "{key}");
    }
    let surfaces = rig.live.control_surfaces.lock().unwrap();
    let keys: Vec<String> = (surfaces["inst"].controls().into_iter())
        .map(|meta| meta.key)
        .collect();
    assert_eq!(
        keys,
        ["release", "note_on", "note_off", "root_note.0", "asset.0", "level.0"]
    );
}
