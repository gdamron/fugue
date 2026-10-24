//! A selection is a request the audio thread acts on, never one it
//! overwrites: `select_cell` reads back the cell last asked for, `cell` the
//! cell playing. The bank is a payload taken whole, the one it replaces
//! handed back to be retired, and neither a selection nor a bank change
//! allocates, frees or waits on a lock.

use super::*;
use crate::alloc_counter::allocator_events;
use crate::control_request::{apply_declared, RtValue};
use crate::payload::{Payload, Retired};

fn bank(cells: usize, steps: usize) -> Bank {
    (0..cells)
        .map(|cell| {
            (0..steps)
                .map(|s| Step::note((cell * 4 + s) as i8))
                .collect()
        })
        .collect()
}

fn select(seq: &mut CellSequencer, cell: i32) {
    apply_declared(seq, table::SELECT_CELL, RtValue::I32(cell)).unwrap();
}

/// One clock edge, a sample high then a sample low.
fn edge(seq: &mut CellSequencer) {
    seq.set_input("clock", 1.0).unwrap();
    seq.process(1);
    seq.set_input("clock", 0.0).unwrap();
    seq.process(1);
}

fn read(seq: &CellSequencer, key: &str) -> f32 {
    seq.get_control(key).unwrap()
}

/// Many blocks of clocked playback with selections, `next_cell` triggers
/// and bank changes landing between them, counted. The banks are built,
/// and the ones they replace freed, outside the count.
#[test]
fn selections_and_bank_changes_neither_allocate_nor_free() {
    let mut seq = CellSequencer::new(1000)
        .with_step_count(4)
        .with_one_shot(true)
        .with_cells(bank(4, 4));
    let mut banks: Vec<Payload> = (1..=16)
        .map(|n| Payload::new(bank(1 + n % 5, 1 + n % 4)))
        .collect();
    let mut retired: Vec<Retired> = Vec::with_capacity(banks.len());
    let ((), allocs, frees) = allocator_events(|| {
        for block in 0..400 {
            for i in 0..64 {
                let clock = if (block * 64 + i) % 50 < 25 { 1.0 } else { 0.0 };
                seq.inputs.block_mut(0)[i] = clock;
            }
            seq.process(64);
            match block % 25 {
                7 => select(&mut seq, (block % 7) as i32),
                13 => apply_declared(&mut seq, table::NEXT_CELL, RtValue::F32(1.0)).unwrap(),
                24 => {
                    let payload = banks.pop().unwrap();
                    retired.push(seq.apply_payload(table::CELLS, payload).unwrap());
                }
                _ => {}
            }
        }
    });
    assert_eq!((allocs, frees), (0, 0));
    assert_eq!(retired.len(), 16);
}

/// The old race: one-shot playback falling into the next cell wrote the
/// cell it fell into back over the selection. A selection that lands as
/// the cycle ends plays, and is still the cell asked for afterwards.
#[test]
fn a_selection_landing_as_one_shot_falls_into_the_next_cell_plays() {
    let mut seq = CellSequencer::new(1000)
        .with_step_count(2)
        .with_one_shot(true)
        .with_cells(bank(4, 2));
    edge(&mut seq);
    edge(&mut seq);
    assert_eq!(seq.current_step(), 1, "the cycle's last step");
    // The selection lands just before the edge that would fall into cell 1.
    select(&mut seq, 3);
    edge(&mut seq);
    assert_eq!(seq.current_cell(), 3, "the selection, not the fall-through");
    assert_eq!(read(&seq, "cell"), 3.0);
    assert_eq!(read(&seq, "select_cell"), 3.0);
    // Playback falls through from the cell asked for: on to the end gate.
    edge(&mut seq);
    edge(&mut seq);
    assert!(seq.finished, "cell 3 was the last");
    assert_eq!(read(&seq, "select_cell"), 3.0, "the request, untouched");
}

/// One-shot falling into the next cell moves `cell`, never `select_cell`:
/// a reader tells the request from what plays.
#[test]
fn falling_through_moves_cell_and_leaves_the_request_alone() {
    let mut seq = CellSequencer::new(1000)
        .with_step_count(2)
        .with_one_shot(true)
        .with_cells(bank(3, 2));
    select(&mut seq, 0);
    for _ in 0..3 {
        edge(&mut seq);
    }
    assert_eq!(read(&seq, "cell"), 1.0, "fell into cell 1");
    assert_eq!(read(&seq, "select_cell"), 0.0, "asked for cell 0");
    // Asking for the cell that plays changes nothing.
    select(&mut seq, 1);
    edge(&mut seq);
    assert_eq!((seq.current_cell(), seq.current_step()), (1, 1));
    // `next_cell` moves on from the cell that plays.
    apply_declared(&mut seq, table::NEXT_CELL, RtValue::F32(1.0)).unwrap();
    seq.process(1);
    assert_eq!(read(&seq, "cell"), 2.0);
    assert_eq!(read(&seq, "select_cell"), 1.0);
    assert_eq!(read(&seq, "next_cell"), 0.0, "a trigger, not a state");
}

/// A selection landing while a deferred one waits replaces it, and one
/// landing just after a deferred one commits is acted on: neither is lost.
#[test]
fn a_selection_around_a_deferred_commit_is_never_lost() {
    let mut seq = CellSequencer::new(1000)
        .with_step_count(2)
        .with_wait_for_cycle_end(true)
        .with_cells(bank(4, 2));
    edge(&mut seq);
    select(&mut seq, 1);
    seq.process(1);
    select(&mut seq, 2);
    seq.process(1);
    assert_eq!(seq.current_cell(), 0, "deferred to the cycle end");
    edge(&mut seq);
    edge(&mut seq);
    assert_eq!(seq.current_cell(), 2, "the later request committed");
    // A request landing right after the commit waits for the next cycle end.
    select(&mut seq, 3);
    edge(&mut seq);
    assert_eq!(seq.current_cell(), 2);
    edge(&mut seq);
    assert_eq!(seq.current_cell(), 3, "committed at the next cycle end");
    assert_eq!(read(&seq, "select_cell"), 3.0);
}

/// A bank change landing beside a selection keeps it, played against the
/// new bank.
#[test]
fn a_bank_change_beside_a_selection_keeps_it() {
    let mut seq = CellSequencer::new(1000)
        .with_step_count(2)
        .with_cells(bank(2, 2));
    edge(&mut seq);
    select(&mut seq, 1);
    let replaced = seq.apply_payload(table::CELLS, Payload::new(bank(3, 2)));
    drop(replaced.unwrap());
    seq.process(1);
    assert_eq!(seq.current_cell(), 1);
    assert_eq!(read(&seq, "cell_count"), 3.0);
    // A bank too short for the playing cell moves it to its last cell.
    select(&mut seq, 2);
    seq.process(1);
    let replaced = seq.apply_payload(table::CELLS, Payload::new(bank(2, 2)));
    drop(replaced.unwrap());
    assert_eq!((seq.current_cell(), read(&seq, "cell")), (1, 1.0));
    assert_eq!(read(&seq, "cell_count"), 2.0);
}

#[test]
fn a_payload_of_another_kind_is_refused_and_handed_back() {
    let mut seq = CellSequencer::new(1000).with_cells(bank(2, 2));
    let Err((refusal, payload)) = seq.apply_payload(table::CELLS, Payload::new(5u8)) else {
        panic!("refused");
    };
    assert_eq!(refusal, Refusal::Invalid);
    assert!(payload.downcast::<u8>().is_ok());
    let Err((refusal, _)) = seq.apply_payload(table::MODE, Payload::new(bank(1, 1))) else {
        panic!("refused");
    };
    assert_eq!(refusal, Refusal::Unsupported);
    assert_eq!(read(&seq, "cell_count"), 2.0);
}

/// Every index names the control the table declares at it.
#[test]
fn control_indices_follow_the_table() {
    let named = [
        (table::ROOT_NOTE, "root_note"),
        (table::STEP_COUNT, "step_count"),
        (table::FOLLOW_CELL_LENGTH, "follow_cell_length"),
        (table::GATE_LENGTH, "gate_length"),
        (table::SELECT_CELL, "select_cell"),
        (table::WAIT_FOR_CYCLE_END, "wait_for_cycle_end"),
        (table::CELLS, "cells"),
        (table::MODE, "mode"),
        (table::LOOP_COUNT, "loop_count"),
        (table::CELL, "cell"),
        (table::CELL_COUNT, "cell_count"),
        (table::NEXT_CELL, "next_cell"),
        (table::GRACE_DURATION, "grace_duration"),
        (table::GRACE_VELOCITY, "grace_velocity"),
        (table::GRACE_PLACEMENT, "grace_placement"),
        (table::ENDED, "ended"),
    ];
    assert_eq!(table::TABLE.len(), named.len());
    for (index, key) in named {
        assert_eq!(table::TABLE.resolve(key), Some(index), "{key}");
    }
}
