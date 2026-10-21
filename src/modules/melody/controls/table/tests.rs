use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::modules::melody::MelodyControls;
use crate::ControlSurface;

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
    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let (controls, done) = (controls.clone(), done.clone());
        let (scales, weights) = (scales.clone(), weights.clone());
        std::thread::spawn(move || {
            for which in (0..2).cycle() {
                if done.load(Ordering::Acquire) {
                    break;
                }
                controls.set_note_weights(weights[which].clone());
                controls.set_allowed_degrees(scales[which].clone());
                std::thread::yield_now();
            }
        })
    };
    let (mut degrees, mut weights_read) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
    let (mut taken, mut refused) = (0, 0);
    while taken < 5_000 {
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
    let _whole = controls.table.writing();
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
