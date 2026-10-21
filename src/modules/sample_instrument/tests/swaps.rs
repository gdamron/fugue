//! An asset swap never makes the audio thread wait on a control thread, or
//! free the audio it replaces.

use std::sync::{mpsc, Arc};
use std::time::Duration;

use super::{build_instrument, write_level_wav, TOL};
use crate::alloc_counter::allocator_events;
use crate::modules::sample_instrument::{SampleInstrument, SampleInstrumentControls, ZoneSpec};
use crate::modules::sample_loading::SampleData;
use crate::{ControlSurface, ControlValue, Module};

/// The swap lock, held by another thread as a control thread mid-
/// `set_asset` would, until [`Self::release`]: or for ten seconds, should
/// the audio thread wait on it, so that fails rather than hangs.
struct HeldSwapLock {
    release: mpsc::Sender<()>,
    holder: std::thread::JoinHandle<bool>,
}

fn hold_swap_lock(controls: &SampleInstrumentControls) -> HeldSwapLock {
    let controls = controls.clone();
    let (release, released) = mpsc::channel();
    let (holding, holds) = mpsc::channel();
    let holder = std::thread::spawn(move || {
        let _held = controls.hold_swap_lock();
        holding.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).is_ok()
    });
    holds.recv_timeout(Duration::from_secs(10)).unwrap();
    HeldSwapLock { release, holder }
}

impl HeldSwapLock {
    fn release(self) {
        let _ = self.release.send(());
        let in_time = self.holder.join().unwrap();
        assert!(in_time, "the audio thread waited on the swap lock");
    }
}

fn note(controls: &SampleInstrumentControls, name: &str, note: f32) {
    controls
        .set_control(name, ControlValue::Number(note))
        .unwrap();
}

#[test]
fn a_busy_swap_lock_defers_the_swap_instead_of_blocking() {
    let original = write_level_wav(0.5, 44_100);
    let replacement = write_level_wav(0.25, 44_100);
    let config = serde_json::json!({
        "zones": [ { "root_note": 60, "key_range": [0, 127], "asset": original.to_str().unwrap() } ]
    });
    let (mut instrument, controls) = build_instrument(config);
    controls
        .set_asset(0, replacement.to_str().unwrap())
        .unwrap();

    // Staged, but the lock is held: the block plays on the old audio.
    let held = hold_swap_lock(&controls);
    note(&controls, "note_on", 60.0);
    instrument.process(1);
    held.release();
    assert!((instrument.get_output("audio_left").unwrap() - 0.5).abs() < TOL);

    // A later block takes the swap: a new note starts on the new audio.
    note(&controls, "note_off", 60.0);
    for _ in 0..44_100 {
        instrument.process(1);
    }
    note(&controls, "note_on", 64.0);
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.25).abs() < TOL);

    let _ = std::fs::remove_file(original);
    let _ = std::fs::remove_file(replacement);
}

#[test]
fn replacing_zone_audio_frees_nothing_on_the_audio_thread() {
    let first = write_level_wav(0.5, 64);
    let second = write_level_wav(0.25, 64);
    let spec = ZoneSpec {
        root_note: 60,
        key_low: 0,
        key_high: 127,
        asset: first.to_str().unwrap().to_string(),
        level: 1.0,
        loop_spec: None,
    };
    let (controls, mut audio, queues) =
        SampleInstrumentControls::new(44_100, 0.01, vec![spec]).unwrap();
    // Audio no cache shares, so dropping it would free its buffers.
    let unique = Arc::new(SampleData::from_interleaved(
        1,
        44_100,
        44_100,
        vec![0.5; 64],
    ));
    let watch = Arc::downgrade(&unique);
    audio[0].sample = unique;
    let mut instrument =
        SampleInstrument::new_with_controls(controls.clone(), audio, queues, 4, 44_100);
    instrument.process(1);

    controls.set_asset(0, second.to_str().unwrap()).unwrap();
    let (_, allocs, frees) = allocator_events(|| instrument.process(1));
    assert_eq!((allocs, frees), (0, 0));
    assert!(watch.upgrade().is_some(), "parked for a control thread");
    note(&controls, "note_on", 60.0);
    instrument.process(1);
    assert!((instrument.get_output("audio_left").unwrap() - 0.25).abs() < TOL);

    // The next swap, on a control thread, drops it.
    controls.set_asset(0, first.to_str().unwrap()).unwrap();
    assert!(watch.upgrade().is_none());

    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
}
