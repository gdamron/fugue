//! Exhaustive interleaving models of the control cells' ordering arguments.
//!
//! The cell source files are compiled a second time here against loom's
//! atomics (the `sync` module below replaces the crate's `std` one), so the
//! models check the exact code that ships and run in a plain
//! `cargo test --lib control_cells::loom_tests`. Each test explores every
//! interleaving and every weak-memory outcome loom models.
// The cell files are deliberately loaded a second time, and not every item
// they define is exercised here.
#![allow(clippy::duplicate_mod, dead_code)]

mod sync {
    pub(super) use loom::sync::atomic::{AtomicU32, Ordering};
    pub(super) use loom::sync::{Mutex, MutexGuard};
    pub(super) use loom::thread::yield_now;
}

#[path = "event.rs"]
mod event;
#[path = "guard.rs"]
mod guard;
#[path = "scalar.rs"]
mod scalar;
#[path = "seq.rs"]
mod seq;
#[path = "value.rs"]
mod value;

use loom::sync::atomic::{AtomicU32, Ordering};
use loom::sync::Arc;
use loom::thread;

use event::{EventCounter, EventCursor};
use guard::ControlLock;
use scalar::ScalarCell;
use seq::{CellIndex, SeqCells, SeqSnapshot};

const PREEMPTIONS: usize = 3;

const A: CellIndex<u32, 2> = CellIndex::new(0);
const B: CellIndex<u32, 2> = CellIndex::new(1);

fn pair(snapshot: &SeqSnapshot<2>) -> (u32, u32) {
    (snapshot.get(A), snapshot.get(B))
}

/// Spawns a control thread that writes `(value, value)` as one edit.
fn spawn_edit(
    cells: &Arc<SeqCells<2>>,
    lock: &Arc<ControlLock>,
    value: u32,
) -> thread::JoinHandle<()> {
    let (cells, lock) = (cells.clone(), lock.clone());
    thread::spawn(move || {
        cells.edit(&lock.lock(), |edit| {
            edit.set(A, value);
            edit.set(B, value);
        });
    })
}

/// (a) + (c): an accepted snapshot is entirely before or entirely after an
/// edit, and a rejected one keeps the previous copy exactly.
#[test]
fn a_snapshot_never_accepts_part_of_an_edit() {
    loom::model(|| {
        let cells = Arc::new(SeqCells::<2>::new());
        let lock = Arc::new(ControlLock::new());
        let mut snapshot = SeqSnapshot::new(&cells);
        cells.store(A, 7);
        cells.store(B, 7);

        let editor = spawn_edit(&cells, &lock, 1);
        let accepted = snapshot.refresh(&cells);
        let seen = pair(&snapshot);
        if accepted {
            assert!(seen == (7, 7) || seen == (1, 1), "torn snapshot {seen:?}");
        } else {
            assert_eq!(seen, (0, 0), "a rejected refresh keeps the previous copy");
        }
        editor.join().unwrap();

        assert!(snapshot.refresh(&cells));
        assert_eq!(pair(&snapshot), (1, 1));
    });
}

/// (b): the audio thread's single-cell store racing an edit lands entirely
/// before or entirely after it, and what the audio thread's snapshot saw
/// agrees with that order.
#[test]
fn a_single_store_lands_before_or_after_an_edit() {
    loom::model(|| {
        let cells = Arc::new(SeqCells::<2>::new());
        let lock = Arc::new(ControlLock::new());
        let mut snapshot = SeqSnapshot::new(&cells);

        let editor = spawn_edit(&cells, &lock, 1);
        cells.store(A, 2);
        let mid = snapshot.refresh(&cells).then(|| pair(&snapshot));
        editor.join().unwrap();

        // Store then edit leaves (1, 1); edit then store leaves (2, 1).
        let end = (cells.load(A), cells.load(B));
        match mid {
            None => assert!(
                end == (1, 1) || end == (2, 1),
                "not a serial order: {end:?}"
            ),
            // Store, snapshot, edit.
            Some((2, 0)) => assert_eq!(end, (1, 1), "the snapshot put the store before the edit"),
            // Store, edit, snapshot.
            Some((1, 1)) => assert_eq!(end, (1, 1), "the snapshot put the store before the edit"),
            // Edit, store, snapshot.
            Some((2, 1)) => assert_eq!(end, (2, 1), "the snapshot put the store after the edit"),
            Some(other) => panic!("snapshot {other:?} matches no serial order"),
        }
        assert!(snapshot.refresh(&cells));
        assert_eq!(pair(&snapshot), end);
    });
}

/// Two edits serialized by one lock never interleave, in the cells or in an
/// accepted snapshot.
#[test]
fn edits_under_one_lock_are_serial() {
    // Three threads and a mutex: bound preemptions to keep this to seconds
    // (unbounded it takes minutes and finds nothing more).
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(PREEMPTIONS);
    model.check(|| {
        let cells = Arc::new(SeqCells::<2>::new());
        let lock = Arc::new(ControlLock::new());
        let mut snapshot = SeqSnapshot::new(&cells);

        let first = spawn_edit(&cells, &lock, 1);
        let second = spawn_edit(&cells, &lock, 2);
        if snapshot.refresh(&cells) {
            let (a, b) = pair(&snapshot);
            assert_eq!(a, b, "torn snapshot");
        }
        first.join().unwrap();
        second.join().unwrap();

        let (a, b) = (cells.load(A), cells.load(B));
        assert!(a == b && a != 0, "edits interleaved: {:?}", (a, b));
    });
}

/// (d): concurrent requests are all counted, and a cursor that sees a
/// request sees what its requester stored before it.
#[test]
fn every_request_is_counted_and_publishes_earlier_stores() {
    loom::model(|| {
        let counter = Arc::new(EventCounter::new());
        let payloads = Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
        let requesters: Vec<_> = (0..2)
            .map(|i| {
                let (counter, payloads) = (counter.clone(), payloads.clone());
                thread::spawn(move || {
                    payloads[i].store(1, Ordering::Relaxed);
                    counter.request();
                })
            })
            .collect();

        let mut cursor = EventCursor::new();
        let seen = cursor.take(&counter);
        let published = payloads
            .iter()
            .filter(|payload| payload.load(Ordering::Relaxed) == 1)
            .count() as u32;
        assert!(
            published >= seen,
            "{seen} requests seen, {published} payloads"
        );

        for requester in requesters {
            requester.join().unwrap();
        }
        assert_eq!(seen + cursor.take(&counter), 2, "a request was lost");
    });
}

/// A scalar cell publishes what its writer stored before it.
#[test]
fn a_scalar_cell_publishes_earlier_stores() {
    loom::model(|| {
        let data = Arc::new(AtomicU32::new(0));
        let ready = Arc::new(ScalarCell::new(false));
        let writer = {
            let (data, ready) = (data.clone(), ready.clone());
            thread::spawn(move || {
                data.store(42, Ordering::Relaxed);
                ready.store(true);
            })
        };
        if ready.load() {
            assert_eq!(data.load(Ordering::Relaxed), 42);
        }
        writer.join().unwrap();
    });
}
