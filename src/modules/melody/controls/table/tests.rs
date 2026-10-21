use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::modules::melody::MelodyControls;
use crate::ControlSurface;

thread_local! {
    /// What runs on this thread between the next two loads that must agree.
    static BETWEEN_LOADS: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

/// Called by the table between a degrees' header load and a weights'
/// generation load, and between a weights' generation load and a count
/// load: where another thread's edit can land.
pub(super) fn between_loads() {
    if let Some(run) = BETWEEN_LOADS.with(|hook| hook.borrow_mut().take()) {
        run();
    }
}

/// Runs `edit` the next time this thread is between two such loads.
fn between_the_next_loads(edit: impl FnOnce() + 'static) {
    BETWEEN_LOADS.with(|hook| *hook.borrow_mut() = Some(Box::new(edit)));
}

/// Waits (up to ten seconds) until `progress` moves `by` past where it was.
fn wait_for(progress: &AtomicUsize, by: usize) {
    let (from, deadline) = (progress.load(Ordering::Acquire), Instant::now());
    while progress.load(Ordering::Acquire) < from + by {
        assert!(deadline.elapsed() < Duration::from_secs(10), "no progress");
        std::thread::yield_now();
    }
}

/// Main's degree table, recomputed whole under its lock after every edit:
/// the reference the atomic table must match.
struct Reference {
    degrees: Vec<i32>,
    weights: Vec<f32>,
    scale: Vec<i32>,
    scale_weights: Vec<f32>,
    count: usize,
    written_degrees: [Option<i32>; MAX_DEGREES],
    written_weights: [Option<f32>; MAX_DEGREES],
}

impl Reference {
    fn new(mut scale: Vec<i32>) -> Self {
        scale.truncate(MAX_DEGREES);
        let mut table = Self {
            degrees: Vec::new(),
            weights: Vec::new(),
            count: scale.len(),
            scale,
            scale_weights: Vec::new(),
            written_degrees: [None; MAX_DEGREES],
            written_weights: [None; MAX_DEGREES],
        };
        table.recompute();
        table
    }

    fn recompute(&mut self) {
        let n = self.scale.len();
        self.degrees = (0..self.count)
            .map(|i| {
                (self.written_degrees[i]).unwrap_or(if n == 0 { 0 } else { self.scale[i % n] })
            })
            .collect();
        self.weights = (0..self.count)
            .map(|i| {
                (self.written_weights[i])
                    .or_else(|| self.scale_weights.get(i).copied())
                    .or_else(|| {
                        (n > 0)
                            .then(|| self.scale_weights.get(i % n).copied())
                            .flatten()
                    })
                    .unwrap_or(1.0)
            })
            .collect();
    }

    fn apply(&mut self, op: &Op) -> bool {
        match op {
            Op::Scale(degrees) => {
                self.scale = degrees.iter().copied().take(MAX_DEGREES).collect();
                self.count = self.scale.len();
                self.written_degrees = [None; MAX_DEGREES];
            }
            Op::Weights(weights) => {
                self.scale_weights = weights.clone();
                self.written_weights = [None; MAX_DEGREES];
            }
            Op::Count(count) => self.count = (*count).clamp(1, MAX_DEGREES),
            Op::Degree(index, value) if *index < self.count => {
                self.written_degrees[*index] = Some((*value).clamp(-127, 127));
            }
            Op::Weight(index, value) if *index < self.count => {
                self.written_weights[*index] = Some(value.clamp(0.0, 10.0));
            }
            Op::Degree(..) | Op::Weight(..) => return false,
            Op::Restore(index, degree, weight) => {
                if *index < MAX_DEGREES {
                    self.written_degrees[*index] = Some((*degree).clamp(-127, 127));
                    self.written_weights[*index] = Some(weight.clamp(0.0, 10.0));
                }
            }
        }
        self.recompute();
        true
    }
}

#[derive(Debug)]
enum Op {
    Scale(Vec<i32>),
    Weights(Vec<f32>),
    Count(usize),
    Degree(usize, i32),
    Weight(usize, f32),
    Restore(usize, i32, f32),
}

fn apply(controls: &MelodyControls, op: &Op) -> bool {
    match op {
        Op::Scale(degrees) => controls.set_allowed_degrees(degrees.clone()),
        Op::Weights(weights) => controls.set_note_weights(weights.clone()),
        Op::Count(count) => controls.set_degree_count(*count),
        Op::Degree(index, value) => return controls.set_degree(*index, *value).is_ok(),
        Op::Weight(index, value) => return controls.set_note_weight(*index, *value).is_ok(),
        Op::Restore(index, degree, weight) => {
            controls.restore_written([(*index, *degree)], [(*index, *weight)]);
        }
    }
    true
}

fn random_op(rng: &mut StdRng) -> Op {
    let index = |rng: &mut StdRng| rng.random_range(0..MAX_DEGREES + 8);
    match rng.random_range(0..6) {
        0 => Op::Scale(
            (0..rng.random_range(0..=MAX_DEGREES + 20))
                .map(|_| rng.random_range(-200..=200))
                .collect(),
        ),
        1 => Op::Weights(
            (0..rng.random_range(0..=MAX_DEGREES + 20))
                .map(|_| rng.random_range(-2.0..12.0))
                .collect(),
        ),
        2 => Op::Count(rng.random_range(0..=MAX_DEGREES + 20)),
        3 => Op::Degree(index(rng), rng.random_range(-200..=200)),
        4 => Op::Weight(index(rng), rng.random_range(-2.0..12.0)),
        _ => Op::Restore(
            index(rng),
            rng.random_range(-200..=200),
            rng.random_range(-2.0..12.0),
        ),
    }
}

fn bits(weights: &[f32]) -> Vec<u32> {
    weights.iter().map(|weight| weight.to_bits()).collect()
}

#[test]
fn the_atomic_table_plays_what_the_locked_one_did_under_random_edits() {
    let mut rng = StdRng::seed_from_u64(0x0393);
    for _ in 0..200 {
        let scale: Vec<i32> = (0..rng.random_range(0..12))
            .map(|_| rng.random_range(-24..=24))
            .collect();
        let (controls, mut reference) = (
            MelodyControls::new(60, scale.clone()),
            Reference::new(scale),
        );
        for _ in 0..40 {
            let op = random_op(&mut rng);
            assert_eq!(apply(&controls, &op), reference.apply(&op), "{op:?}");
            assert_eq!(controls.allowed_degrees(), reference.degrees, "{op:?}");
            assert_eq!(
                bits(&controls.note_weights()),
                bits(&reference.weights),
                "{op:?}"
            );
            assert_eq!(controls.degree_count(), reference.count);
        }
    }
}

#[test]
fn a_copy_taken_while_whole_scales_are_set_is_always_one_of_them() {
    let scales = [
        (0..MAX_DEGREES as i32).collect::<Vec<_>>(),
        vec![1, 3, 6, 8, 10],
    ];
    let controls = MelodyControls::new(60, scales[1].clone());
    let weights = [vec![2.0; MAX_DEGREES], vec![0.5; 5]];
    let (done, edits) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    let writer = {
        let (controls, done, edits) = (controls.clone(), done.clone(), edits.clone());
        let (scales, weights) = (scales.clone(), weights.clone());
        std::thread::spawn(move || {
            for which in (0..2).cycle() {
                if done.load(Ordering::Acquire) {
                    break;
                }
                controls.set_note_weights(weights[which].clone());
                controls.set_allowed_degrees(scales[which].clone());
                edits.fetch_add(1, Ordering::Release);
                std::thread::yield_now();
            }
        })
    };
    wait_for(&edits, 1);
    let (mut degrees, mut weights_read) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
    let (mut taken, mut refused) = (0, 0);
    let (started, deadline) = (edits.load(Ordering::Acquire), Instant::now());
    while taken < 5_000 || edits.load(Ordering::Acquire) < started + 500 {
        assert!(deadline.elapsed() < Duration::from_secs(10), "no edits");
        let Some(count) = controls.table.read_into(&mut degrees, &mut weights_read) else {
            refused += 1;
            continue;
        };
        taken += 1;
        let copied = &degrees[..count];
        assert!(
            scales.iter().any(|scale| scale[..] == *copied),
            "half of one scale and half of another: {copied:?}"
        );
        for weight in &weights_read[..count] {
            assert!([2.0, 0.5, 1.0].contains(weight), "a torn weight: {weight}");
        }
    }
    done.store(true, Ordering::Release);
    writer.join().unwrap();
    eprintln!("copies taken {taken}, refused mid-edit {refused}");
}

#[test]
fn a_position_write_racing_a_whole_scale_lands_before_it_or_after_it() {
    let controls = MelodyControls::new(60, vec![0]);
    let table = &controls.table;
    // A write that read the table before the scale was set, and stores
    // after: checked against the count then, so position 1 is refused...
    let before = table.header.load(Ordering::Acquire);
    controls.set_allowed_degrees(vec![0, 7]);
    assert!(!table.write_degree_as_of(before, 1, 11));
    // ...and position 0 lands before the scale, forgotten with the rest.
    assert!(table.write_degree_as_of(before, 0, 11));
    assert_eq!(controls.allowed_degrees(), [0, 7]);
    // A write that read the table after it is kept.
    controls.set_degree(1, 11).unwrap();
    assert_eq!(controls.allowed_degrees(), [0, 11]);
    // Whole weights forget written weights the same way.
    controls.set_note_weight(1, 3.0).unwrap();
    controls.set_note_weights(vec![2.0]);
    assert_eq!(controls.note_weights(), [2.0, 1.0]);
}

#[test]
fn a_position_reads_on_the_audio_thread_without_a_lock() {
    let controls = MelodyControls::new(60, vec![0, 2, 4]);
    controls.set_note_weight(1, 3.0).unwrap();
    let _writing = controls.table.writing();
    let audio = AudioThreadScope::enter();
    let (read, allocs, frees) = allocator_events(|| {
        [
            controls.get_control("degree.1"),
            controls.get_control("note_weight.1"),
        ]
    });
    drop(audio);
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(read, [Ok(2.0.into()), Ok(3.0.into())]);
}

#[test]
fn a_stale_write_never_replaces_one_made_after_a_whole_edit() {
    let controls = MelodyControls::new(60, vec![0]);
    let table = &controls.table;
    // A write that checked the generation, then was preempted: meanwhile
    // the scale is set whole and a position written. It stores after.
    let before = table.degrees_generation();
    let preempted = Cell::new(true);
    let now = || {
        if preempted.replace(false) {
            controls.set_allowed_degrees(vec![0, 7]);
            controls.set_degree(0, 5).unwrap();
            return before;
        }
        table.degrees_generation()
    };
    table.write(&table.written_degrees[0], before, now, 11);
    assert_eq!(controls.allowed_degrees(), [5, 7]);
    // Weights likewise.
    let before = table.weights_generation();
    let preempted = Cell::new(true);
    let now = || {
        if preempted.replace(false) {
            controls.set_note_weights(vec![1.0, 1.0]);
            controls.set_note_weight(0, 4.0).unwrap();
            return before;
        }
        table.weights_generation()
    };
    table.write(&table.written_weights[0], before, now, 9.0f32.to_bits());
    assert_eq!(controls.note_weights(), [4.0, 1.0]);
}

#[test]
fn a_copy_meeting_a_count_change_and_whole_weights_is_a_table_that_was() {
    let controls = MelodyControls::new(60, vec![0, 2, 4]);
    let table = &controls.table;
    // Between the copy's header load and its weights load, the count
    // shrinks and whole weights are set.
    let edit = controls.clone();
    between_the_next_loads(move || {
        edit.set_degree_count(1);
        edit.set_note_weights(vec![0.0, 0.0, 10.0]);
    });
    let (mut degrees, mut weights) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
    assert_eq!(table.read_into(&mut degrees, &mut weights), None);
    assert_eq!(table.read_into(&mut degrees, &mut weights), Some(1));
    assert_eq!((degrees[0], weights[0]), (0, 0.0));
    // A position read meeting the same retries, and finds it past the
    // count.
    controls.set_degree_count(3);
    controls.set_note_weights(vec![]);
    let edit = controls.clone();
    between_the_next_loads(move || {
        edit.set_degree_count(1);
        edit.set_note_weights(vec![0.0, 0.0, 10.0]);
    });
    assert_eq!(table.position(2), None);
}

#[test]
fn a_weight_write_meeting_whole_weights_and_a_grown_count_is_kept() {
    let controls = MelodyControls::new(60, vec![0]);
    // Between the write's generation load and its count load, whole
    // weights are set and the count grows to take position 5.
    let edit = controls.clone();
    between_the_next_loads(move || {
        edit.set_note_weights(vec![]);
        edit.set_degree_count(6);
    });
    controls.set_note_weight(5, 9.0).unwrap();
    assert_eq!(controls.note_weights()[5], 9.0);
}

#[test]
fn a_read_meeting_an_editor_preempted_mid_edit_reads_the_table_before_it() {
    let controls = MelodyControls::new(60, vec![0, 2]);
    controls.set_degree_count(4);
    controls.set_note_weights(vec![3.0]);
    let table = &controls.table;
    // An editor setting [7, 9, 11, 13] with weights [5.0], preempted after
    // its first entries.
    let _whole = table.whole();
    let next = next_generation(table.degrees_generation());
    table.scales[buffer(next)].entries[0].store(7, Ordering::Relaxed);
    table.scales[buffer(next)].len.store(4, Ordering::Relaxed);
    let next = next_generation(table.weights_generation());
    let weight = 5.0f32.to_bits();
    table.weights[buffer(next)].entries[0].store(weight, Ordering::Relaxed);
    let audio = AudioThreadScope::enter();
    let read: Vec<_> = (0..4).map(|i| table.position(i)).collect();
    let mut degrees = [0; MAX_DEGREES];
    let mut weights = [0.0; MAX_DEGREES];
    let copied = table.read_into(&mut degrees, &mut weights);
    drop(audio);
    let before = [(0, 3.0), (2, 1.0), (0, 3.0), (2, 1.0)];
    assert_eq!(read, before.map(Some));
    assert_eq!(copied, Some(4));
    assert_eq!(degrees[..4], [0, 2, 0, 2]);
    assert_eq!(weights[..4], [3.0, 1.0, 3.0, 1.0]);
}

#[test]
fn a_copy_is_not_taken_while_a_position_write_is_in_flight() {
    let controls = MelodyControls::new(60, vec![0, 0]);
    let (mut degrees, mut weights) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
    let writing = controls.table.writing();
    assert_eq!(controls.table.read_into(&mut degrees, &mut weights), None);
    drop(writing);
    assert_eq!(
        controls.table.read_into(&mut degrees, &mut weights),
        Some(2)
    );
}

#[test]
fn a_copy_never_holds_a_later_position_write_without_an_earlier_one() {
    let controls = MelodyControls::new(60, vec![-127, -127]);
    let (done, writes) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    let writer = {
        let (controls, done, writes) = (controls.clone(), done.clone(), writes.clone());
        std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                for value in -127..=127 {
                    controls.set_degree(0, value).unwrap();
                    controls.set_degree(1, value).unwrap();
                    writes.fetch_add(1, Ordering::Release);
                }
            }
        })
    };
    wait_for(&writes, 1);
    let (mut degrees, mut weights) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
    let mut taken = 0;
    let (started, deadline) = (writes.load(Ordering::Acquire), Instant::now());
    while taken < 20_000 || writes.load(Ordering::Acquire) < started + 5_000 {
        assert!(deadline.elapsed() < Duration::from_secs(10), "no writes");
        if controls
            .table
            .read_into(&mut degrees, &mut weights)
            .is_none()
        {
            continue;
        }
        taken += 1;
        // Position 0 is written first: it leads position 1 by one value, or
        // has wrapped round to -127 from 127.
        let [first, second] = [degrees[0], degrees[1]];
        let held = first == second || first == second + 1 || (first, second) == (-127, 127);
        assert!(held, "a table that never was: [{first}, {second}]");
    }
    done.store(true, Ordering::Release);
    writer.join().unwrap();
}

#[test]
fn generations_alternate_buffers_and_skip_never_written_round_the_wrap() {
    assert_eq!(next_generation(1), 2);
    assert_eq!(next_generation(u32::MAX), 2);
    for generation in [1, 2, u32::MAX - 1, u32::MAX] {
        let next = next_generation(generation);
        assert_ne!(next, 0);
        assert_ne!(buffer(next), buffer(generation));
    }
}

#[test]
fn a_forgotten_write_stays_forgotten_when_its_generation_comes_back_round() {
    let controls = MelodyControls::new(60, vec![0, 2]);
    let table = &controls.table;
    // Near the wrap: generation u32::MAX - 1 (buffer 0, as 2 is).
    let near = header(u32::MAX - 1, 2);
    table.scales[0].set(&[0, 2]);
    table.header.store(near, Ordering::Release);
    table
        .weights_generation
        .store(u32::MAX - 1, Ordering::Release);
    controls.set_degree(0, 9).unwrap();
    controls.set_note_weight(0, 6.0).unwrap();
    // Two whole edits round the wrap; a write stored by a writer that
    // checked the generation before the first and stored after it.
    controls.set_allowed_degrees(vec![0, 2]);
    controls.set_note_weights(vec![]);
    let stale = tagged(u32::MAX, 4);
    table.written_degrees[1].store(stale, Ordering::Release);
    controls.set_allowed_degrees(vec![0, 2]);
    controls.set_note_weights(vec![]);
    assert_eq!(generation_of(table.header.load(Ordering::Acquire)), 2);
    // The generations come back round: nothing written then is read.
    for generation in [u32::MAX - 1, u32::MAX] {
        table.header.store(header(generation, 2), Ordering::Release);
        table.scales[buffer(generation)].set(&[0, 2]);
        table
            .weights_generation
            .store(generation, Ordering::Release);
        table.weights[buffer(generation)].set(&[]);
        assert_eq!(controls.allowed_degrees(), [0, 2]);
        assert_eq!(controls.note_weights(), [1.0, 1.0]);
    }
}
