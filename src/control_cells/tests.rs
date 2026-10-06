//! Unit and allocation tests for the control cells. The interleaving
//! arguments are modelled exhaustively in `loom_tests`.

use std::panic::{catch_unwind, AssertUnwindSafe};

use super::*;
use crate::alloc_counter::allocator_events;

/// A small enum stored by tag; unknown tags decode to the default.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    Sine,
    Square,
}

impl CellValue for Shape {
    fn to_bits(self) -> u32 {
        self as u32
    }
    fn from_bits(bits: u32) -> Self {
        match bits {
            1 => Shape::Square,
            _ => Shape::Sine,
        }
    }
}

/// Melody's shape: degrees, weights and a count in one group.
const MAX: usize = 128;
const CELLS: usize = 2 * MAX + 1;
const DEGREES: CellRange<i32, CELLS> = CellRange::new(0, MAX);
const WEIGHTS: CellRange<f32, CELLS> = CellRange::new(MAX, MAX);
const COUNT: CellIndex<u32, CELLS> = CellIndex::new(2 * MAX);

const A: CellIndex<u32, 2> = CellIndex::new(0);
const B: CellIndex<u32, 2> = CellIndex::new(1);

#[test]
fn values_round_trip_through_cell_bits() {
    for value in [0.0f32, -0.0, 1.5, f32::INFINITY, f32::MIN_POSITIVE] {
        assert_eq!(
            f32::from_bits(CellValue::to_bits(value)).to_bits(),
            value.to_bits()
        );
    }
    assert!(<f32 as CellValue>::from_bits(CellValue::to_bits(f32::NAN)).is_nan());
    for value in [i32::MIN, -7, 0, 7, i32::MAX] {
        assert_eq!(<i32 as CellValue>::from_bits(value.to_bits()), value);
    }
    assert_eq!(<u32 as CellValue>::from_bits(u32::MAX), u32::MAX);
    assert!(<bool as CellValue>::from_bits(true.to_bits()));
    assert!(!<bool as CellValue>::from_bits(false.to_bits()));
    assert!(
        <bool as CellValue>::from_bits(7),
        "any nonzero bits decode as true"
    );
    assert_eq!(Shape::from_bits(Shape::Square.to_bits()), Shape::Square);
    assert_eq!(
        Shape::from_bits(99),
        Shape::Sine,
        "invalid tags decode, not panic"
    );
}

#[test]
fn scalar_cell_loads_what_was_stored() {
    let cell = ScalarCell::new(Shape::Sine);
    assert_eq!(cell.load(), Shape::Sine);
    cell.store(Shape::Square);
    assert_eq!(cell.load(), Shape::Square);
    let gain = ScalarCell::new(0.25f32);
    gain.store(-1.5);
    assert_eq!(gain.load(), -1.5);
}

#[test]
fn ranges_address_their_own_cells() {
    let cells = SeqCells::<CELLS>::new();
    let lock = ControlLock::new();
    cells.edit(&lock.lock(), |edit| {
        for i in 0..DEGREES.len() {
            edit.set(DEGREES.at(i), -(i as i32));
            edit.set(WEIGHTS.at(i), i as f32 / 2.0);
        }
        edit.set(COUNT, 5);
    });
    let snapshot = SeqSnapshot::new(&cells);
    assert_eq!(snapshot.get(DEGREES.at(MAX - 1)), -(MAX as i32 - 1));
    assert_eq!(snapshot.get(WEIGHTS.at(3)), 1.5);
    assert_eq!(snapshot.get(COUNT), 5);
    assert_eq!(cells.load(WEIGHTS.at(0)), 0.0);
}

#[test]
#[should_panic(expected = "out of range")]
fn range_index_past_len_panics() {
    let _ = DEGREES.at(MAX);
}

#[test]
fn snapshot_sees_single_stores_and_finished_edits() {
    let cells = SeqCells::<2>::new();
    let lock = ControlLock::new();
    let mut snapshot = SeqSnapshot::new(&cells);
    cells.store(A, 3);
    assert_eq!(cells.load(A), 3);
    assert_eq!(snapshot.get(A), 0, "a snapshot only changes on refresh");
    assert!(snapshot.refresh(&cells));
    assert_eq!((snapshot.get(A), snapshot.get(B)), (3, 0));
    cells.edit(&lock.lock(), |edit| {
        edit.set(A, 1);
        edit.set(B, 2);
    });
    assert!(snapshot.refresh(&cells));
    assert_eq!((snapshot.get(A), snapshot.get(B)), (1, 2));
}

#[test]
fn snapshot_rejects_while_an_edit_is_in_flight_and_keeps_the_previous_copy() {
    let cells = SeqCells::<2>::new();
    let lock = ControlLock::new();
    let mut snapshot = SeqSnapshot::new(&cells);
    cells.edit(&lock.lock(), |edit| {
        edit.set(A, 1);
        cells.store(B, 9);
        assert!(!snapshot.refresh(&cells), "odd sequence: reject");
        assert_eq!((snapshot.get(A), snapshot.get(B)), (0, 0));
        edit.set(B, 2);
    });
    assert!(snapshot.refresh(&cells));
    assert_eq!(
        (snapshot.get(A), snapshot.get(B)),
        (1, 2),
        "the edit's blind write wins"
    );
}

#[test]
fn a_panicking_edit_still_ends_the_edit() {
    let cells = SeqCells::<2>::new();
    let lock = ControlLock::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let guard = lock.lock();
        cells.edit(&guard, |edit| {
            edit.set(A, 4);
            panic!("edit failed");
        });
    }));
    assert!(result.is_err());
    // The panic poisoned the lock's mutex; the lock ignores that.
    cells.edit(&lock.lock(), |edit| edit.set(B, 5));
    let snapshot = SeqSnapshot::new(&cells);
    assert_eq!((snapshot.get(A), snapshot.get(B)), (4, 5));
}

#[test]
fn event_cursor_counts_every_request_once() {
    let counter = EventCounter::new();
    let mut cursor = EventCursor::new();
    assert_eq!(counter.count(), 0);
    counter.request();
    counter.request();
    assert_eq!(cursor.take(&counter), 2);
    assert_eq!(cursor.take(&counter), 0);
    counter.request();
    assert_eq!(cursor.take(&counter), 1);
}

#[test]
fn event_cursor_counts_across_the_wrap() {
    let counter = EventCounter::starting_at(u32::MAX - 1);
    let mut cursor = EventCursor::new();
    assert_eq!(cursor.take(&counter), u32::MAX - 1);
    counter.request();
    counter.request();
    counter.request();
    assert_eq!(counter.count(), 1);
    assert_eq!(cursor.take(&counter), 3);
}

#[test]
fn audio_side_operations_never_allocate_or_free() {
    let scalar = ScalarCell::new(0.5f32);
    let cells = SeqCells::<CELLS>::new();
    let lock = ControlLock::new();
    let mut snapshot = SeqSnapshot::new(&cells);
    let counter = EventCounter::new();
    let mut cursor = EventCursor::new();

    let (sum, allocs, frees) = allocator_events(|| {
        let _scope = AudioBlockScope::enter();
        scalar.store(scalar.load() * 2.0);
        cells.store(DEGREES.at(3), -2);
        cells.store(COUNT, 4);
        counter.request();
        let accepted = snapshot.refresh(&cells);
        let taken = cursor.take(&counter);
        assert!(accepted);
        cells.load(COUNT) + snapshot.get(COUNT) + taken
    });
    assert_eq!((sum, allocs, frees), (9, 0, 0));

    cells.edit(&lock.lock(), |edit| {
        edit.set(COUNT, 7);
        let (accepted, allocs, frees) = allocator_events(|| snapshot.refresh(&cells));
        assert_eq!((accepted, allocs, frees), (false, 0, 0), "reject path");
    });
    assert_eq!(snapshot.get(COUNT), 4);
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "ControlLock::lock called inside process_block")]
fn locking_inside_an_audio_block_panics_in_debug() {
    let lock = ControlLock::new();
    let _scope = AudioBlockScope::enter();
    let _guard = lock.lock();
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "SeqCells::edit called inside process_block")]
fn editing_inside_an_audio_block_panics_in_debug() {
    let cells = SeqCells::<2>::new();
    let lock = ControlLock::new();
    let guard = lock.lock();
    let _scope = AudioBlockScope::enter();
    cells.edit(&guard, |edit| edit.set(A, 1));
}

#[cfg(debug_assertions)]
#[test]
fn audio_block_scopes_nest_and_restore() {
    let lock = ControlLock::new();
    let outer = AudioBlockScope::enter();
    let inner = AudioBlockScope::enter();
    drop(inner);
    let still_inside = catch_unwind(AssertUnwindSafe(|| drop(lock.lock())));
    assert!(
        still_inside.is_err(),
        "leaving the inner scope keeps the outer one"
    );
    drop(outer);
    drop(lock.lock());
}

/// Tries to take a control lock from inside `process()`, recording whether
/// the debug check refused.
#[cfg(debug_assertions)]
struct LocksInProcess {
    refused: std::sync::Arc<std::sync::atomic::AtomicBool>,
    buffer: [f32; crate::MAX_BLOCK],
}

#[cfg(debug_assertions)]
impl crate::Module for LocksInProcess {
    fn name(&self) -> &str {
        "locks_in_process"
    }
    fn process(&mut self, _frames: usize) -> bool {
        let lock = ControlLock::new();
        let refused = catch_unwind(AssertUnwindSafe(|| drop(lock.lock()))).is_err();
        self.refused
            .store(refused, std::sync::atomic::Ordering::Relaxed);
        true
    }
    fn inputs(&self) -> &[&str] {
        &[]
    }
    fn outputs(&self) -> &[&str] {
        &[]
    }
    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut self.buffer
    }
    fn output_block(&self, _index: usize) -> &[f32] {
        &self.buffer
    }
    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("no input {port}"))
    }
    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("no output {port}"))
    }
}

#[cfg(debug_assertions)]
#[test]
fn process_block_refuses_control_locks_in_debug() {
    use crate::invention::graph::{MasterObservers, SignalGraph};
    let refused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let module = LocksInProcess {
        refused: refused.clone(),
        buffer: [0.0; crate::MAX_BLOCK],
    };
    let mut modules = indexmap::IndexMap::new();
    modules.insert(
        "locker".to_string(),
        crate::factory::GraphModule::Module(Box::new(module)),
    );
    let mut graph = SignalGraph::new(modules, Vec::new(), Vec::new(), MasterObservers::default());
    graph.recompile();
    let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
    graph.process_block(&mut left, &mut right);
    assert!(refused.load(std::sync::atomic::Ordering::Relaxed));
    drop(ControlLock::new().lock());
}
