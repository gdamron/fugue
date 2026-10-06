//! Single-position and count controls while the degree table is busy: the
//! deferred-write mailbox, and a `control_scheduler` ramping them from the
//! audio thread.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use indexmap::IndexMap;

use super::*;
use crate::modules::control_scheduler::{
    ControlScheduler, ControlSchedulerControls, ScheduleEntry, SurfaceDirectory,
};
use crate::ControlSurface;

fn controls() -> MelodyControls {
    MelodyControls::new(60, vec![0, 2, 4, 5])
}

/// The table as it stands, read without draining the mailbox.
fn table_weights(ctrl: &MelodyControls) -> Vec<f32> {
    ctrl.table.lock().unwrap().weights.clone()
}

fn table_degrees(ctrl: &MelodyControls) -> Vec<i32> {
    ctrl.table.lock().unwrap().degrees.clone()
}

#[test]
fn the_last_deferred_write_to_a_slot_wins() {
    let ctrl = controls();
    let held = ctrl.table.lock().unwrap();
    ctrl.set_note_weight(0, 2.0).unwrap();
    ctrl.set_note_weight(0, 3.0).unwrap();
    ctrl.set_degree(1, 5).unwrap();
    ctrl.set_degree(1, 6).unwrap();
    // Contended reads answer from the mailbox, then from the mirror.
    assert_eq!(ctrl.note_weight(0).unwrap(), 3.0);
    assert_eq!(ctrl.degree(1).unwrap(), 6);
    assert_eq!(ctrl.degree(2).unwrap(), 4);
    assert_eq!(ctrl.degree_count(), 4);
    assert!(ctrl.has_pending());
    drop(held);

    assert_eq!(ctrl.note_weights(), [3.0, 1.0, 1.0, 1.0]);
    assert_eq!(ctrl.allowed_degrees(), [0, 6, 4, 5]);
    assert!(!ctrl.has_pending());
}

#[test]
fn contended_reads_and_writes_refuse_the_same_indices() {
    let ctrl = controls();
    let uncontended = (
        ctrl.degree(4).unwrap_err(),
        ctrl.set_note_weight(4, 1.0).unwrap_err(),
    );
    let held = ctrl.table.lock().unwrap();
    assert_eq!(ctrl.degree(4).unwrap_err(), uncontended.0);
    assert_eq!(ctrl.set_note_weight(4, 1.0).unwrap_err(), uncontended.1);
    assert!(!ctrl.has_pending());
    drop(held);
}

#[test]
fn a_direct_write_after_a_deposit_is_not_overwritten_by_it() {
    let ctrl = controls();
    let held = ctrl.table.lock().unwrap();
    ctrl.set_note_weight(0, 2.0).unwrap();
    drop(held);

    // Locking drains the older deposit before this write applies.
    ctrl.set_note_weight(0, 4.0).unwrap();
    assert_eq!(table_weights(&ctrl)[0], 4.0);
    assert_eq!(ctrl.note_weight(0).unwrap(), 4.0);
}

#[test]
fn a_whole_table_set_after_a_deposit_wins() {
    let ctrl = controls();
    let held = ctrl.table.lock().unwrap();
    ctrl.set_note_weight(0, 2.0).unwrap();
    ctrl.set_degree(1, 9).unwrap();
    drop(held);

    ctrl.set_note_weights(vec![5.0, 6.0]);
    ctrl.set_allowed_degrees(vec![0, 2, 4, 5]);
    assert_eq!(ctrl.note_weights(), [5.0, 6.0, 1.0, 1.0]);
    assert_eq!(ctrl.allowed_degrees(), [0, 2, 4, 5]);
}

#[test]
fn a_deposit_made_while_a_holder_edits_lands_when_it_releases() {
    // Whole-table edits hold the lock through this guard.
    let ctrl = controls();
    let version = ctrl.table_version();
    let guard = ctrl.lock_table();
    ctrl.set_note_weight(2, 7.0).unwrap();
    assert!(ctrl.has_pending());
    drop(guard);

    // Applied by the holder's post-release check, with no other access.
    assert!(!ctrl.has_pending());
    assert_ne!(ctrl.table_version(), version);
    assert_eq!(table_weights(&ctrl), [1.0, 1.0, 7.0, 1.0]);
}

#[test]
fn a_deferred_count_growth_reveals_positions_for_deferred_writes() {
    let ctrl = MelodyControls::new(60, vec![0, 2, 4]);
    let held = ctrl.table.lock().unwrap();
    ctrl.set_degree_count(5);
    assert_eq!(ctrl.degree_count(), 5);
    ctrl.set_degree(4, 9).unwrap();
    assert_eq!(ctrl.degree(4).unwrap(), 9);
    assert_eq!(
        ctrl.set_degree(5, 1).unwrap_err(),
        "Degree index 5 out of range (count: 5)"
    );
    drop(held);

    assert_eq!(ctrl.allowed_degrees(), [0, 2, 4, 0, 9]);
}

#[test]
fn a_deferred_shrink_keeps_an_earlier_deferred_write_hidden() {
    let ctrl = controls();
    let held = ctrl.table.lock().unwrap();
    ctrl.set_degree(3, 11).unwrap();
    ctrl.set_degree_count(2);
    assert!(ctrl.degree(3).is_err());
    drop(held);

    assert_eq!(ctrl.allowed_degrees(), [0, 2]);
    // As if written before the shrink: growing shows it again.
    ctrl.set_degree_count(4);
    assert_eq!(ctrl.allowed_degrees(), [0, 2, 4, 11]);
}

// The tests below hold the guard and run an edit's body from the holder's
// side, so a deposit lands deterministically while the edit holds the lock.
// They call the same `DegreeTable` methods (`replace_degrees`,
// `replace_weights`, `set_count`) and `publish` that `set_allowed_degrees`,
// `set_note_weights` and `set_degree_count` run under it, flags included.

/// An 8-degree melody, as a document might configure one.
fn octave() -> MelodyControls {
    MelodyControls::new(60, vec![0, 2, 4, 5, 7, 9, 11, 12])
}

#[test]
fn a_weight_deferred_during_a_scale_replacement_is_kept_hidden() {
    let ctrl = octave();
    let mut guard = ctrl.lock_table();
    ctrl.set_note_weight(6, 4.0).unwrap();
    guard.replace_degrees(vec![0, 4, 7]);
    ctrl.pending.publish(&guard);
    drop(guard);

    assert_eq!(table_weights(&ctrl), [1.0; 3]);
    // Replacing the scale keeps written weights, so growing shows it.
    ctrl.set_degree_count(7);
    assert_eq!(ctrl.note_weights()[6], 4.0);
}

#[test]
fn writes_deferred_during_a_direct_count_shrink_are_kept_hidden() {
    let ctrl = octave();
    let mut guard = ctrl.lock_table();
    ctrl.set_degree(5, 3).unwrap();
    ctrl.set_note_weight(5, 4.0).unwrap();
    guard.set_count(2);
    ctrl.pending.publish(&guard);
    drop(guard);

    assert_eq!(table_degrees(&ctrl), [0, 2]);
    ctrl.set_degree_count(6);
    assert_eq!(ctrl.allowed_degrees(), [0, 2, 4, 5, 7, 3]);
    assert_eq!(ctrl.note_weights()[5], 4.0);
}

#[test]
fn a_scale_replacement_erases_a_degree_deferred_past_a_deferred_shrink() {
    let ctrl = octave();
    let mut guard = ctrl.lock_table();
    ctrl.set_degree(5, 3).unwrap();
    ctrl.set_degree_count(2);
    guard.replace_degrees(vec![0, 4, 7]);
    ctrl.pending.publish(&guard);
    drop(guard);

    assert_eq!(table_degrees(&ctrl), [0, 4]);
    ctrl.set_degree_count(6);
    assert_eq!(ctrl.allowed_degrees(), [0, 4, 7, 0, 4, 7]);
}

#[test]
fn a_weight_replacement_erases_only_deferred_weights() {
    let ctrl = octave();
    let mut guard = ctrl.lock_table();
    ctrl.set_degree(5, 3).unwrap();
    ctrl.set_note_weight(5, 4.0).unwrap();
    ctrl.set_degree_count(2);
    guard.replace_weights(vec![2.0]);
    ctrl.pending.publish(&guard);
    drop(guard);

    ctrl.set_degree_count(6);
    assert_eq!(ctrl.allowed_degrees(), [0, 2, 4, 5, 7, 3]);
    assert_eq!(ctrl.note_weights(), [2.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn an_earlier_replacement_does_not_erase_a_later_deferred_write() {
    let ctrl = MelodyControls::new(60, vec![]);
    // Replaced with no deposit pending: nothing drains on its release.
    ctrl.set_allowed_degrees(vec![0, 2, 4, 5, 7, 9, 11, 12]);

    // A later, separate direct count shrink; its holder's drain on acquire
    // resets the flags first.
    let mut guard = ctrl.lock_table();
    ctrl.set_degree(5, 3).unwrap();
    guard.set_count(2);
    ctrl.pending.publish(&guard);
    drop(guard);

    ctrl.set_degree_count(6);
    assert_eq!(ctrl.allowed_degrees(), [0, 2, 4, 5, 7, 3]);
}

#[test]
fn concurrent_position_and_whole_table_edits_settle_on_each_threads_last_write() {
    const ROUNDS: usize = 20_000;
    let ctrl = controls();
    let whole = {
        let ctrl = ctrl.clone();
        thread::spawn(move || {
            for i in 0..ROUNDS {
                // Keeps weights; forgets degrees written to single positions.
                ctrl.set_allowed_degrees(vec![0, 2, 4, (i % 12) as i32]);
                ctrl.set_note_weight(2, (i % 10) as f32).unwrap();
            }
        })
    };
    let positions = {
        let ctrl = ctrl.clone();
        thread::spawn(move || {
            for i in 0..ROUNDS {
                ctrl.set_note_weight(0, (i % 7) as f32).unwrap();
                ctrl.set_note_weight(1, (i % 9) as f32).unwrap();
                let _ = ctrl.degree(1);
            }
        })
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        whole.join().unwrap();
        positions.join().unwrap();
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(20))
        .expect("the writers deadlocked");

    // Nothing is stranded: whatever the latency-only retries left pending,
    // the next lock holder drains (as the melody's next block would).
    drop(ctrl.lock_table());
    assert!(!ctrl.has_pending());
    let last = ROUNDS - 1;
    assert_eq!(
        table_weights(&ctrl),
        [
            (last % 7) as f32,
            (last % 9) as f32,
            (last % 10) as f32,
            1.0
        ]
    );
    assert_eq!(table_degrees(&ctrl), [0, 2, 4, (last % 12) as i32]);
}

/// A scheduler over `melody` (registered as `melody`), adopted and ready for
/// the audio thread.
fn scheduler_over(melody: &MelodyControls, schedule: &str) -> ControlScheduler {
    let mut map = IndexMap::new();
    map.insert(
        "melody".to_string(),
        Arc::new(melody.clone()) as Arc<dyn ControlSurface + Send + Sync>,
    );
    let directory: SurfaceDirectory = Arc::new(Mutex::new(map));
    let spec: Vec<ScheduleEntry> = serde_json::from_str(schedule).unwrap();
    let ctrl = ControlSchedulerControls::new(spec);
    ctrl.attach("sched", &directory).unwrap();
    let mut scheduler = ControlScheduler::new(48_000, ctrl);
    scheduler.prepare_for_publication();
    scheduler
}

/// One gate rising edge, then `low_frames` low frames, a frame at a time.
fn pulse(scheduler: &mut ControlScheduler, low_frames: usize) {
    scheduler.set_input("gate", 1.0).unwrap();
    scheduler.process(1);
    scheduler.set_input("gate", 0.0).unwrap();
    for _ in 0..low_frames {
        scheduler.process(1);
    }
}

/// Runs `pulses` gate steps of 16 frames on a worker thread while this
/// thread holds the degree table lock, failing (not hanging) if the
/// scheduler blocks on it.
fn run_while_held(melody: &MelodyControls, mut scheduler: ControlScheduler, pulses: usize) {
    let held = melody.table.lock().unwrap();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        for _ in 0..pulses {
            pulse(&mut scheduler, 15);
        }
        tx.send(()).unwrap();
    });
    let finished = rx.recv_timeout(Duration::from_secs(5));
    drop(held);
    finished.expect("the scheduler's block blocked on the degree table lock");
    worker.join().unwrap();
    // Released without the guard, so the deferred writes are still pending.
    assert!(melody.has_pending(), "writes were deferred while held");
}

#[test]
fn a_scheduler_ramp_over_a_busy_degree_table_never_blocks() {
    let melody = MelodyControls::new(60, vec![0, 7]);
    melody.set_note_weights(vec![1.0, 0.0]);
    melody.set_seed(1);
    let mut generator = MelodyGenerator::new(melody.clone());
    assert_eq!(generator.next_note().midi_note, 60);

    // Two-step ramps, each sample between edges and on both edges.
    let scheduler = scheduler_over(
        &melody,
        r#"[
            { "at": 0, "module": "melody", "control": "note_weight.0", "value": 5.0, "ramp": 2 },
            { "at": 0, "module": "melody", "control": "degree.0", "value": 12.0, "ramp": 2 }
        ]"#,
    );
    let version = melody.table_version();
    run_while_held(&melody, scheduler, 3);

    // The melody's next block drains the last ramp values and copies them.
    generator.process(1);
    assert!(!melody.has_pending());
    assert_ne!(melody.table_version(), version);
    assert_eq!(table_weights(&melody), [5.0, 0.0]);
    assert_eq!(table_degrees(&melody), [12, 7]);
    assert_eq!(generator.next_note().midi_note, 72);
}

#[test]
fn a_ramp_fired_while_the_table_is_busy_starts_from_the_latest_values() {
    let melody = controls();
    melody.set_degree(0, 4).unwrap();
    let scheduler = scheduler_over(
        &melody,
        r#"[
            { "at": 0, "module": "melody", "control": "note_weight.0", "value": 5.0, "ramp": 4 },
            { "at": 0, "module": "melody", "control": "degree.0", "value": 8.0, "ramp": 4 }
        ]"#,
    );
    let held = melody.table.lock().unwrap();
    // Deferred, so the weight ramp starts from the mailbox; the degree ramp
    // starts from the mirror.
    melody.set_note_weight(0, 3.0).unwrap();
    // Released without the guard, so the deposit is still pending.
    drop(held);
    assert!(melody.has_pending());

    run_while_held(&melody, scheduler, 2);
    // The last write lands 15/16 of the way through the second of four
    // steps: from + (to - from) * (1 + 15/16) / 4.
    let progress = (1.0 + 15.0 / 16.0) / 4.0;
    assert_eq!(melody.note_weight(0).unwrap(), 3.0 + 2.0 * progress);
    assert_eq!(melody.degree(0).unwrap(), (4.0 + 4.0 * progress) as i32);
}

#[test]
fn scheduler_ramps_over_melody_controls_do_not_allocate() {
    let schedule = r#"[
        { "at": 0, "module": "melody", "control": "note_weight.0", "value": 5.0, "ramp": 4 },
        { "at": 0, "module": "melody", "control": "degree.0", "value": 12.0, "ramp": 4 }
    ]"#;

    // Uncontended.
    let melody = controls();
    let mut scheduler = scheduler_over(&melody, schedule);
    let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
        for _ in 0..3 {
            pulse(&mut scheduler, 15);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(melody.note_weight(0).unwrap() > 1.0);

    // With the lock held elsewhere: ramp starts, deposits and retries.
    let melody = controls();
    let mut generator = MelodyGenerator::new(melody.clone());
    let mut scheduler = scheduler_over(&melody, schedule);
    let held = melody.table.lock().unwrap();
    let ((), allocs, frees) = crate::alloc_counter::allocator_events(|| {
        for _ in 0..3 {
            pulse(&mut scheduler, 15);
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert!(melody.has_pending());
    drop(held);

    // The melody's block drains the deferred writes without allocating.
    let (_, allocs, frees) = crate::alloc_counter::allocator_events(|| generator.process(64));
    assert_eq!((allocs, frees), (0, 0));
    assert!(!melody.has_pending());
    assert!(table_weights(&melody)[0] > 1.0);
}
