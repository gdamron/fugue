//! A fixed group of control cells published together under a seqlock.

use std::marker::PhantomData;

use super::guard::{debug_assert_off_audio_thread, ControlGuard};
use super::sync::{yield_now, AtomicU32, Ordering};
use super::value::CellValue;

/// The position of one `T`-typed cell in a [`SeqCells<N>`].
///
/// Construction checks `index < N`, so an out-of-range `const` handle fails
/// to compile, and every access through a handle is in bounds.
#[derive(Clone, Copy)]
pub(crate) struct CellIndex<T, const N: usize> {
    index: usize,
    _type: PhantomData<fn() -> T>,
}

impl<T: CellValue, const N: usize> CellIndex<T, N> {
    /// # Panics
    ///
    /// If `index >= N`: a compile error when evaluated in a `const`.
    pub(crate) const fn new(index: usize) -> Self {
        assert!(index < N, "cell index out of range");
        Self {
            index,
            _type: PhantomData,
        }
    }
}

/// `len` consecutive `T`-typed cells of a [`SeqCells<N>`] from `start`, so a
/// module can name a table once:
/// `const DEGREES: CellRange<i32, CELLS> = CellRange::new(0, MAX_DEGREES);`.
#[derive(Clone, Copy)]
pub(crate) struct CellRange<T, const N: usize> {
    start: usize,
    len: usize,
    _type: PhantomData<fn() -> T>,
}

impl<T: CellValue, const N: usize> CellRange<T, N> {
    /// # Panics
    ///
    /// If the range ends past `N`: a compile error when evaluated in a `const`.
    pub(crate) const fn new(start: usize, len: usize) -> Self {
        assert!(start <= N && len <= N - start, "cell range out of bounds");
        Self {
            start,
            len,
            _type: PhantomData,
        }
    }

    pub(crate) const fn len(self) -> usize {
        self.len
    }

    /// The `i`th cell of the range.
    ///
    /// # Panics
    ///
    /// If `i >= len`. Control keys are validated when they are resolved
    /// (R5), so on the audio thread this check never fails in normal use.
    #[inline]
    pub(crate) fn at(self, i: usize) -> CellIndex<T, N> {
        assert!(i < self.len, "cell {i} out of range (len {})", self.len);
        CellIndex {
            index: self.start + i,
            _type: PhantomData,
        }
    }
}

/// `N` control cells that control threads edit together and the audio thread
/// reads as one consistent [`SeqSnapshot`], under a seqlock.
///
/// Cells hold raw `u32` bits; each access site picks the type through a
/// [`CellIndex`] or [`CellRange`], so one group can mix `i32` degrees,
/// `f32` weights and a `u32` count.
///
/// - [`store`](Self::store) and [`load`](Self::load) touch one cell from
///   any thread, including the audio thread: lock-free and allocation-free.
///   Stores do not touch the sequence.
/// - [`edit`](Self::edit) writes several cells from a control thread holding
///   a [`ControlGuard`], bracketed by an odd/even sequence.
/// - [`SeqSnapshot::refresh`] copies all cells on the audio thread and keeps
///   the copy only if no edit overlapped it.
///
/// Writer role: each cell is a parameter or telemetry, as for
/// [`ScalarCell`](super::ScalarCell) (R1); edits write parameters only.
///
/// # Ordering
///
/// An edit takes the sequence from even `s` to odd `s + 1` (compare-exchange,
/// `Acquire`, so it follows the previous edit), swaps its values into the
/// cells with `Release`, then stores even `s + 2` with `Release`. A snapshot
/// loads the sequence, every cell and the sequence again, all `Acquire`.
///
/// - All or nothing: if the copy holds a value an edit wrote, that `Release`
///   makes the edit's `s + 1` happen-before the second sequence load, which
///   therefore cannot return the even `s` the first load saw. If the first
///   load saw `s + 2`, all the edit's writes happen-before the copy, which
///   sees them or later values (and a later edit's values are rejected as
///   above). So an accepted copy holds all of an edit or none of it.
/// - Single stores: [`store`](Self::store) is a `swap(AcqRel)`, so a store
///   that overwrites an edit's write (directly or through other stores)
///   acquires it and passes the edge above on: a snapshot that sees the store
///   cannot also accept pre-edit values. (A plain store could, and the loom
///   model finds it.) The swap and the edit's write to that cell are ordered
///   in its modification order: store first, the edit overwrites it (store,
///   then edit); edit first, the store survives (edit, then store). Cells the
///   edit does not write commute with it. This needs *blind* edits:
///   [`SeqEdit`] can only set cells, and edits compute their values from
///   control-side state, never from the cells.
/// - Edits swap rather than store only for the loom models. C11 already
///   orders a plain store after a swap that did not read it (RMW atomicity),
///   but loom tracks modification order only through what each write has
///   read, and without the swap explores orders C11 forbids.
///
/// The sequence is a `u32`: a false accept would need 2^31 edits during one
/// snapshot copy.
pub(crate) struct SeqCells<const N: usize> {
    seq: AtomicU32,
    cells: [AtomicU32; N],
}

impl<const N: usize> SeqCells<N> {
    /// A group with every cell's bits zero (`0`, `0.0`, `false`).
    pub(crate) fn new() -> Self {
        Self {
            seq: AtomicU32::new(0),
            cells: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    /// Stores one cell from any thread. See the type's ordering notes for why
    /// this is a `swap`.
    #[inline]
    pub(crate) fn store<T: CellValue>(&self, idx: CellIndex<T, N>, value: T) {
        self.cells[idx.index].swap(value.to_bits(), Ordering::AcqRel);
    }

    /// Loads one cell from any thread. Not consistent with other cells.
    #[inline]
    pub(crate) fn load<T: CellValue>(&self, idx: CellIndex<T, N>) -> T {
        T::from_bits(self.cells[idx.index].load(Ordering::Acquire))
    }

    /// Writes several cells as one edit that snapshots see whole or not at
    /// all. Control threads only; `f` must make blind writes (see the type's
    /// ordering notes).
    ///
    /// Editors of one group should share one [`ControlLock`](super::ControlLock),
    /// so this never waits; if two edits overlap anyway, the second spins
    /// until the first finishes. If `f` panics, the writes it made are
    /// published as the edit.
    ///
    /// # Panics
    ///
    /// In debug builds, if called inside `SignalGraph::process_block`, or if
    /// another edit of this group is in flight.
    pub(crate) fn edit(&self, _guard: &ControlGuard<'_>, f: impl FnOnce(&mut SeqEdit<'_, N>)) {
        debug_assert_off_audio_thread("SeqCells::edit");
        let start = loop {
            let seq = self.seq.load(Ordering::Relaxed);
            let odd = seq.wrapping_add(1);
            if seq.is_multiple_of(2)
                && (self.seq)
                    .compare_exchange(seq, odd, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            {
                break seq;
            }
            if cfg!(debug_assertions) {
                panic!(
                    "overlapping SeqCells edits: editors of one group must share one ControlLock"
                );
            }
            yield_now();
        };
        // Restores an even sequence when `f` returns or panics, so snapshots
        // never stall.
        let _publish = Publish {
            seq: &self.seq,
            even: start.wrapping_add(2),
        };
        f(&mut SeqEdit { cells: &self.cells });
    }
}

/// Ends an edit by publishing the next even sequence, on return or unwind.
struct Publish<'a> {
    seq: &'a AtomicU32,
    even: u32,
}

impl Drop for Publish<'_> {
    fn drop(&mut self) {
        self.seq.store(self.even, Ordering::Release);
    }
}

/// Write access to a [`SeqCells`] during an edit. Write-only by design: the
/// linearizability argument needs blind writes.
pub(crate) struct SeqEdit<'a, const N: usize> {
    cells: &'a [AtomicU32; N],
}

impl<const N: usize> SeqEdit<'_, N> {
    #[inline]
    pub(crate) fn set<T: CellValue>(&mut self, idx: CellIndex<T, N>, value: T) {
        self.cells[idx.index].swap(value.to_bits(), Ordering::Release);
    }
}

/// The audio thread's consistent copy of a [`SeqCells<N>`], held inline (no
/// heap).
///
/// While an edit is in flight, [`refresh`](Self::refresh) keeps the previous
/// copy, so the audio side may lag by a block, including for its own
/// single-cell stores into the group.
pub(crate) struct SeqSnapshot<const N: usize> {
    bits: [u32; N],
}

impl<const N: usize> SeqSnapshot<N> {
    /// Control thread: takes a first consistent copy, waiting out any edit
    /// in flight.
    pub(crate) fn new(cells: &SeqCells<N>) -> Self {
        let mut snapshot = Self { bits: [0; N] };
        while !snapshot.refresh(cells) {
            yield_now();
        }
        snapshot
    }

    /// Copies the cells and keeps the copy if no edit overlapped it,
    /// returning whether it did. Otherwise the previous copy stays. Never
    /// spins, locks or allocates.
    #[inline]
    pub(crate) fn refresh(&mut self, cells: &SeqCells<N>) -> bool {
        let before = cells.seq.load(Ordering::Acquire);
        if !before.is_multiple_of(2) {
            return false;
        }
        let mut copy = [0u32; N];
        for (bits, cell) in copy.iter_mut().zip(&cells.cells) {
            *bits = cell.load(Ordering::Acquire);
        }
        if cells.seq.load(Ordering::Acquire) != before {
            return false;
        }
        self.bits = copy;
        true
    }

    #[inline]
    pub(crate) fn get<T: CellValue>(&self, idx: CellIndex<T, N>) -> T {
        T::from_bits(self.bits[idx.index])
    }
}
