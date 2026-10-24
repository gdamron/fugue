//! A schedule's entries and ramps into every legacy family play on the
//! audio thread without allocating, freeing or waiting on a lock: every
//! control a schedule can write is lock-free at the source, so a write
//! lands whatever a control thread holds or sends meanwhile.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::control_request::{scheduled_surface, while_held};
use crate::invention::runtime::ControlSurfaceInstance;
use crate::module_config::tests::registry::eight_frame_wav;
use crate::modules::sample_instrument::{LoopSpec, SampleInstrument, ZoneSpec};
use crate::modules::{SampleInstrumentControls, SamplePlayerControls};
use crate::{ControlValue, Module};

/// The families that kept state behind locks, kept to hold what locks they
/// still have, with the instrument that plays the notes scheduled to it.
struct Locking {
    player: SamplePlayerControls,
    instrument: SampleInstrumentControls,
    sounding: SampleInstrument,
}

impl Locking {
    fn new() -> Self {
        let wav = eight_frame_wav();
        let zone = ZoneSpec {
            root_note: 60,
            key_low: 0,
            key_high: 127,
            asset: wav.to_string(),
            level: 1.0,
            // Sustained, so a note sounds until it is released.
            loop_spec: Some(LoopSpec {
                start_frames: 2,
                end_frames: 8,
                crossfade_frames: 0,
            }),
        };
        let (instrument, audio, notes) =
            SampleInstrumentControls::new(48_000, 0.1, vec![zone]).unwrap();
        let sounding =
            SampleInstrument::new_with_controls(instrument.clone(), audio, notes, 8, 48_000);
        Self {
            player: SamplePlayerControls::with_mode(48_000, Some(wav), None, None, false).unwrap(),
            instrument,
            sounding,
        }
    }

    /// A directory of every legacy family: the atomic ones fresh, the
    /// locking ones these.
    fn directory(&self) -> SurfaceDirectory {
        let mut map: IndexMap<String, ControlSurfaceInstance> = IndexMap::new();
        map.insert("sample_slicer".into(), scheduled_surface("sample_slicer"));
        map.insert("sample_player".into(), Arc::new(self.player.clone()));
        map.insert(
            "sample_instrument".into(),
            Arc::new(self.instrument.clone()),
        );
        Arc::new(Mutex::new(map))
    }

    /// One block with the clock at `gate`, the scheduler then the
    /// instrument, on the audio thread, counted.
    fn block(&mut self, scheduler: &mut ControlScheduler, gate: f32) -> (usize, usize) {
        scheduler.set_input("clock", gate).unwrap();
        let _audio = AudioThreadScope::enter();
        let (_, allocs, frees) = allocator_events(|| {
            scheduler.process(64);
            self.sounding.process(64);
        });
        (allocs, frees)
    }
}

fn entry(at_step: u64, module: &str, control: &str, value: ScheduleValue) -> ScheduleEntry {
    ScheduleEntry {
        at_step,
        module: module.into(),
        control: control.into(),
        value,
        ramp_steps: None,
    }
}

fn ramp(module: &str, control: &str, to: f32) -> ScheduleEntry {
    ScheduleEntry {
        ramp_steps: Some(2),
        ..entry(0, module, control, ScheduleValue::Number(to))
    }
}

/// A scheduler over `directory` running `entries`.
fn scheduler(entries: Vec<ScheduleEntry>, directory: &SurfaceDirectory) -> ControlScheduler {
    let controls = ControlSchedulerControls::new(entries);
    controls.attach("sched", directory).unwrap();
    let mut scheduler = ControlScheduler::new(48_000, controls);
    scheduler.settle();
    scheduler
}

fn number(directory: &SurfaceDirectory, module: &str, control: &str) -> f32 {
    let surface = directory.lock().unwrap()[module].clone();
    match surface.get_control(control).unwrap() {
        ControlValue::Number(number) => number,
        other => panic!("{module}.{control}: {other:?}"),
    }
}

/// Runs `f` while a thread calls `work` over and over, as a control thread
/// busy with the same module would.
fn while_busy<R>(work: impl Fn() + Sync, f: impl FnOnce() -> R) -> R {
    /// Stops the worker however `f` ends, so a failing assertion fails the
    /// test rather than hanging it.
    struct Stop<'a>(&'a AtomicBool);
    impl Drop for Stop<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                work();
            }
        });
        let _stop = Stop(&done);
        f()
    })
}

#[test]
fn a_failure_while_busy_stops_the_worker() {
    let failed = std::panic::catch_unwind(|| while_busy(|| {}, || panic!("a failed assertion")));
    assert!(failed.is_err());
}

#[test]
fn entries_and_ramps_into_every_legacy_family_allocate_nothing() {
    use ScheduleValue::{Bool, Number};
    let mut locking = Locking::new();
    let directory = locking.directory();
    let entries = vec![
        entry(0, "sample_slicer", "time_ratio", Number(2.0)),
        entry(0, "sample_player", "play", Bool(true)),
        ramp("sample_player", "pitch_ratio", 2.0),
        entry(0, "sample_instrument", "note_on", Number(64.0)),
        ramp("sample_instrument", "level.0", 0.5),
    ];
    let mut scheduler = scheduler(entries, &directory);
    for gate in [1.0, 0.0, 1.0, 0.0, 1.0, 0.0] {
        assert_eq!(locking.block(&mut scheduler, gate), (0, 0));
    }
    for (module, control, to) in [
        ("sample_slicer", "time_ratio", 2.0),
        ("sample_player", "pitch_ratio", 2.0),
        ("sample_instrument", "level.0", 0.5),
    ] {
        let landed = number(&directory, module, control);
        assert!((landed - to).abs() < 1e-4, "{module}.{control}: {landed}");
    }
    assert!(locking.player.play());
    assert_eq!(locking.sounding.sounding(), [64]);
}

#[test]
fn a_write_lands_while_control_threads_hold_every_lock_left() {
    use ScheduleValue::{Bool, Number};
    let mut locking = Locking::new();
    let directory = locking.directory();
    let mut scheduler = scheduler(
        vec![
            entry(1, "sample_player", "loop", Bool(true)),
            entry(1, "sample_instrument", "note_on", Number(64.0)),
        ],
        &directory,
    );
    assert_eq!(locking.block(&mut scheduler, 1.0), (0, 0), "step 0");
    assert_eq!(locking.block(&mut scheduler, 0.0), (0, 0));
    // Step 1 plays while control threads hold every lock these families
    // have left: the player's and the instrument's sample swaps.
    let (player, instrument) = (locking.player.clone(), locking.instrument.clone());
    let held = while_held(
        || player.hold_swap_lock(),
        || {
            while_held(
                || instrument.hold_swap_lock(),
                || locking.block(&mut scheduler, 1.0),
            )
        },
    );
    assert_eq!(held, (0, 0), "step 1");
    assert!(locking.player.loop_enabled());
    assert_eq!(locking.sounding.sounding(), [64]);
}

#[test]
fn a_scheduled_chord_plays_in_order_while_a_control_thread_sends_notes() {
    use ScheduleValue::Number;
    let mut locking = Locking::new();
    let directory = locking.directory();
    let chord = [60, 64, 67];
    let entries = (chord.iter())
        .map(|&note| entry(1, "sample_instrument", "note_on", Number(note as f32)))
        .collect();
    let mut scheduler = scheduler(entries, &directory);
    let instrument = locking.instrument.clone();
    let other = ControlValue::Number(72.0);
    let started = while_busy(
        || drop(instrument.note_on(&other)),
        || {
            assert_eq!(locking.block(&mut scheduler, 1.0), (0, 0), "step 0");
            assert_eq!(locking.block(&mut scheduler, 0.0), (0, 0));
            assert_eq!(locking.block(&mut scheduler, 1.0), (0, 0), "step 1");
            locking.sounding.sounding()
        },
    );
    let played: Vec<u8> = (started.into_iter())
        .filter(|note| chord.contains(note))
        .collect();
    assert_eq!(
        played, chord,
        "every note of the chord, in order, on its step"
    );
}
