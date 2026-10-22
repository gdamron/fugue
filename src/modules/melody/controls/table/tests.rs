use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::alloc_counter::allocator_events;
use crate::audio_thread::AudioThreadScope;
use crate::modules::melody::tests::without_waiting;
use crate::modules::melody::MelodyControls;

mod reference;
mod restore;
mod support;
mod threads;

// Tests whose hooks make whole edits run in `without_waiting`: a hooked
// edit meeting a read or write that held the edit lock would deadlock the
// test thread, which nothing else could interrupt.

use support::*;
pub(super) use support::{between_loads, still_waiting};

/// The table `scale` and `weights` make, set one after the other.
fn table_of(scale: &[i32], weights: Option<&[f32]>) -> (Vec<i32>, Vec<f32>) {
    let controls = MelodyControls::new(60, scale.to_vec());
    if let Some(weights) = weights {
        controls.set_note_weights(weights.to_vec());
    }
    (controls.allowed_degrees(), controls.note_weights())
}

#[test]
fn a_copy_taken_while_whole_scales_are_set_is_always_one_of_them() {
    // Three, of different lengths and values, so each buffer is rewritten
    // with different contents as the generations alternate between two.
    let scales = [
        (0..MAX_DEGREES as i32).collect::<Vec<_>>(),
        vec![1, 3, 6, 8, 10],
        vec![2, 4, 5, 7, 9, 11, 12],
    ];
    let weights = [vec![2.0; MAX_DEGREES], vec![0.5; 5], vec![3.0, 4.0, 5.0]];
    // The writer sets weights k, then scale k: every table it passes
    // through, whole.
    let mut tables = vec![table_of(&scales[2], None)];
    for k in 0..3 {
        tables.push(table_of(&scales[(k + 2) % 3], Some(&weights[k])));
        tables.push(table_of(&scales[k], Some(&weights[k])));
    }
    let controls = MelodyControls::new(60, scales[2].clone());
    let (done, edits) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicUsize::new(0)),
    );
    let writer = {
        let (controls, done, edits) = (controls.clone(), done.clone(), edits.clone());
        let (scales, weights) = (scales.clone(), weights.clone());
        std::thread::spawn(move || {
            for k in (0..3).cycle() {
                if done.load(Ordering::Acquire) {
                    break;
                }
                controls.set_note_weights(weights[k].clone());
                controls.set_allowed_degrees(scales[k].clone());
                edits.fetch_add(1, Ordering::Release);
                std::thread::yield_now();
            }
        })
    };
    wait_for(&edits, 1);
    let stop = Stop(done);
    let (taken, refused) = without_waiting(move || {
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
            let copied = (&degrees[..count], &weights_read[..count]);
            assert!(
                tables.iter().any(|(d, w)| (&d[..], &w[..]) == copied),
                "a table no whole edit made: {copied:?}"
            );
        }
        (taken, refused)
    });
    drop(stop);
    join_soon(writer);
    eprintln!("copies taken {taken}, refused mid-edit {refused}");
}

#[test]
fn a_copy_whose_buffer_is_rewritten_under_it_is_refused() {
    without_waiting(|| {
        let controls = MelodyControls::new(60, vec![1, 2, 3]);
        // Between the copy's header load and its read, two whole scales: the
        // second rewrites the buffer the copy's generation reads.
        let edit = controls.clone();
        between_the_next_loads(move || {
            edit.set_allowed_degrees(vec![4, 5]);
            edit.set_allowed_degrees(vec![6, 7, 8, 9]);
        });
        let (mut degrees, mut weights) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
        let table = &controls.table;
        assert_eq!(table.read_into(&mut degrees, &mut weights), None);
        assert_eq!(table.read_into(&mut degrees, &mut weights), Some(4));
        assert_eq!(degrees[..4], [6, 7, 8, 9]);
    });
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
    let _editing = controls.table.editing();
    let reader = controls.clone();
    let (read, allocs, frees) = without_waiting(move || {
        let _audio = AudioThreadScope::enter();
        allocator_events(|| reader.table.position(1))
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(read, Some((2, 3.0)));
}

#[test]
fn a_position_read_meeting_an_edit_every_attempt_gives_up_without_a_lock() {
    let controls = MelodyControls::new(60, vec![0, 2, 4]);
    let _editing = controls.table.editing();
    let reader = controls.clone();
    let read = without_waiting(move || {
        // A count change published between the loads of every attempt,
        // while a control thread holds the edit lock.
        let edit = reader.clone();
        between_each_of_the_next_loads(2 * READ_ATTEMPTS, move || {
            let table = &edit.table;
            table.set_count(if table.count() == 2 { 3 } else { 2 });
        });
        let _audio = AudioThreadScope::enter();
        let read = reader.table.position(1);
        between_each_of_the_next_loads(0, || ());
        read
    });
    assert_eq!(read, None);
}

#[test]
fn a_stale_write_never_replaces_one_made_after_a_whole_edit() {
    without_waiting(|| {
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
    });
}

#[test]
fn a_copy_meeting_a_count_change_and_whole_weights_is_a_table_that_was() {
    without_waiting(|| {
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
    });
}

#[test]
fn a_weight_write_meeting_whole_weights_and_a_grown_count_is_kept() {
    without_waiting(|| {
        let controls = MelodyControls::new(60, vec![0]);
        // Past generation 2, whose edit waits out writes in flight, this one
        // included.
        controls.set_note_weights(vec![]);
        // Between the write's generation load and its count load, whole
        // weights are set and the count grows to take position 5.
        let edit = controls.clone();
        between_the_next_loads(move || {
            edit.set_note_weights(vec![]);
            edit.set_degree_count(6);
        });
        controls.set_note_weight(5, 9.0).unwrap();
        assert_eq!(controls.note_weights()[5], 9.0);
    });
}

#[test]
fn generations_2_and_2_31_wait_out_a_held_up_write_however_later_ones_finish() {
    // Before generation 2, and before generation 2^31.
    for before in [u32::MAX, (1 << 31) - 1] {
        let controls = MelodyControls::new(60, vec![0, 0]);
        let table = &controls.table;
        // A round of whole scales later, generation `before` holds two
        // positions; a write checks position 1 against it, and is held
        // up.
        table.jump_to(before);
        let (held, release) = held_write(&controls, 1, 9);
        // The next generation comes with one position: had the write gone
        // ahead after it, position 1 would show it once the count grows
        // again, a round later.
        let epoch = table.epoch.load(Ordering::SeqCst);
        let editor = {
            let controls = controls.clone();
            std::thread::spawn(move || controls.set_allowed_degrees(vec![5]))
        };
        let deadline = Instant::now() + PATIENCE;
        while table.epoch.load(Ordering::SeqCst) == epoch {
            assert!(Instant::now() < deadline, "the edit never waited");
            std::thread::yield_now();
        }
        // A write starting and finishing meanwhile does not stand in for
        // the one held up.
        assert!(table.write_degree(0, 3));
        std::thread::sleep(Duration::from_millis(200));
        assert!(!editor.is_finished(), "{before}: the edit waits it out");
        drop(release);
        assert!(join_soon(held));
        join_soon(editor);
    }
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
    let reader = controls.clone();
    let (read, copied, degrees, weights) = without_waiting(move || {
        let _audio = AudioThreadScope::enter();
        let read: Vec<_> = (0..4).map(|i| reader.table.position(i)).collect();
        let mut degrees = [0; MAX_DEGREES];
        let mut weights = [0.0; MAX_DEGREES];
        let copied = reader.table.read_into(&mut degrees, &mut weights);
        (read, copied, degrees, weights)
    });
    let before = [(0, 3.0), (2, 1.0), (0, 3.0), (2, 1.0)];
    assert_eq!(read, before.map(Some));
    assert_eq!(copied, Some(4));
    assert_eq!(degrees[..4], [0, 2, 0, 2]);
    assert_eq!(weights[..4], [3.0, 1.0, 3.0, 1.0]);
}

#[test]
fn a_copy_is_taken_while_a_position_write_is_in_flight() {
    let controls = MelodyControls::new(60, vec![0, 0]);
    let table = &controls.table;
    let (mut degrees, mut weights) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
    // A write held up in flight holds up no copy, nor a scheduled write
    // finishing meanwhile: the copy holds that one.
    let (held, release) = held_write(&controls, 1, 9);
    assert!(table.write_degree(0, 7));
    assert_eq!(table.read_into(&mut degrees, &mut weights), Some(2));
    assert_eq!(degrees[..2], [7, 0]);
    drop(release);
    assert!(join_soon(held));
    assert_eq!(table.read_into(&mut degrees, &mut weights), Some(2));
    assert_eq!(degrees[..2], [7, 9]);
}

#[test]
fn a_copy_never_holds_a_write_made_after_seeing_one_it_missed() {
    without_waiting(|| {
        let controls = MelodyControls::new(60, vec![0, 0]);
        let table = &controls.table;
        // The copy reads position 0. Then a write to it lands, held up before
        // it finishes; this thread reads it, and another write, to position 1,
        // lands, held up likewise. The copy reads position 1: had it kept
        // that, it would hold the later write and not the earlier.
        let writes = Rc::new(RefCell::new(Vec::new()));
        let (edit, held) = (controls.clone(), writes.clone());
        // Past the copy's view, to its first position read.
        between_loads_after(1, move || {
            let first = landed_write(&edit, 0, 20);
            assert_eq!(edit.table.position(0), Some((20, 1.0)));
            let second = landed_write(&edit, 1, 21);
            held.borrow_mut().extend([first, second]);
        });
        let (mut degrees, mut weights) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
        assert_eq!(table.read_into(&mut degrees, &mut weights), None);
        for (write, release) in writes.take() {
            drop(release);
            assert!(join_soon(write));
        }
        assert_eq!(table.read_into(&mut degrees, &mut weights), Some(2));
        assert_eq!(degrees[..2], [20, 21]);
    });
}

#[test]
fn a_copy_begun_while_a_position_write_is_landing_is_refused() {
    let controls = MelodyControls::new(60, vec![0, 0]);
    let table = &controls.table;
    let (mut degrees, mut weights) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
    // Two writes about to land, their versions bumped. Were copies taken
    // now, one could read position 0 before the first lands and position
    // 1 after the second, and another both between: each would hold one
    // write and not the other, in opposite orders.
    let (first, land_first, first_landed) = landing_write(&controls, 0, 10);
    let (second, land_second, second_landed) = landing_write(&controls, 1, 20);
    assert_eq!(table.read_into(&mut degrees, &mut weights), None);
    land_first.send(()).unwrap();
    first_landed
        .recv_timeout(PATIENCE)
        .expect("the first write lands");
    assert_eq!(table.read_into(&mut degrees, &mut weights), None);
    land_second.send(()).unwrap();
    second_landed
        .recv_timeout(PATIENCE)
        .expect("the second write lands");
    // Both landed, neither finished: none is landing, so a copy is the
    // table as it stands.
    assert_eq!(table.read_into(&mut degrees, &mut weights), Some(2));
    assert_eq!(degrees[..2], [10, 20]);
    drop((land_first, land_second));
    assert!(join_soon(first));
    assert!(join_soon(second));
}

#[test]
fn a_copy_during_which_a_position_write_finishes_is_refused() {
    without_waiting(|| {
        let controls = MelodyControls::new(60, vec![0, 0]);
        let table = &controls.table;
        let edit = controls.clone();
        between_the_next_loads(move || edit.set_degree(1, 5).unwrap());
        let (mut degrees, mut weights) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
        assert_eq!(table.read_into(&mut degrees, &mut weights), None);
        assert_eq!(table.read_into(&mut degrees, &mut weights), Some(2));
        assert_eq!(degrees[..2], [0, 5]);
    });
}

#[test]
fn a_read_outlasting_a_round_of_generations_is_refused() {
    without_waiting(|| {
        let controls = MelodyControls::new(60, vec![1, 1]);
        let table = &controls.table;
        // Between the read's header load and its weights load, two whole scales
        // rewrite the buffer it reads, and the generation comes back round to
        // the one it loaded, as a round of them would.
        let generation = table.degrees_generation();
        let edit = controls.clone();
        between_the_next_loads(move || {
            edit.set_allowed_degrees(vec![2, 2]);
            edit.set_allowed_degrees(vec![2, 2]);
            edit.table.jump_to(generation);
        });
        let (mut degrees, mut weights) = ([0; MAX_DEGREES], [0.0; MAX_DEGREES]);
        assert_eq!(table.read_into(&mut degrees, &mut weights), None);
        let edit = controls.clone();
        between_the_next_loads(move || {
            edit.set_allowed_degrees(vec![3, 3]);
            edit.set_allowed_degrees(vec![3, 3]);
            edit.table.jump_to(generation);
        });
        assert_eq!(table.consistent(1, false, |view| view.position(0)), None);
    });
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
    let stop = Stop(done);
    without_waiting(move || {
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
            // Position 0 is written first: it leads position 1 by one value,
            // or has wrapped round to -127 from 127.
            let [first, second] = [degrees[0], degrees[1]];
            let held = first == second || first == second + 1 || (first, second) == (-127, 127);
            assert!(held, "a table that never was: [{first}, {second}]");
        }
    });
    drop(stop);
    join_soon(writer);
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
