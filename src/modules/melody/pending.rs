//! Non-blocking access to the melody's degree table.
//!
//! A `control_scheduler` writes `degree.N`, `note_weight.N` and
//! `degree_count` from the audio thread, once per sample while a ramp runs,
//! and samples a ramp's start value with a read. Those calls must never wait
//! on the degree table's `Mutex` while a control thread (an authored write, a
//! reload, `describe_module`) holds it.
//!
//! So single-position and count writes `try_lock` the table, and when it is
//! busy they deposit the write into a [`Pending`] mailbox instead: one
//! preallocated, lock-free slot per position for a degree and a weight, and
//! one for the count. A later deposit to the same slot overwrites an older
//! one, so the last write wins.
//!
//! # Draining
//!
//! Every holder of the table lock goes through [`TableGuard`], which drains
//! the mailbox when it acquires the lock (so a direct write lands after older
//! deferred ones, and a whole-table edit overwrites them rather than being
//! overwritten) and, after it releases the lock, re-checks the mailbox and
//! drains it again with `try_lock` if a deposit arrived meanwhile. A
//! depositor also retries `try_lock` once after depositing.
//!
//! Invariant: a deposit made while any holder holds the lock is drained by
//! that holder's post-release check or by a later holder, never stranded.
//! The depositor sets the pending flag and then (after a `SeqCst` fence)
//! tries the lock; the holder releases the lock and then (after a `SeqCst`
//! fence) reads the flag. With both fences, either the depositor's retry
//! finds the lock free or the holder sees the flag; if the holder's own
//! `try_lock` then fails, whoever holds the lock now repeats the check on
//! its release.
//!
//! # Reads while contended
//!
//! Reads that find the table busy answer from the mailbox (the pending value
//! for that slot, if any) or else from a lock-free mirror of the effective
//! table, refreshed under the lock after every edit. One known gap: a
//! position revealed by a count growth that is still pending reads its
//! mirror, which may be stale (it holds whatever the position last played,
//! or 0) until the growth is drained.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{fence, AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{MutexGuard, TryLockError};

use super::controls::{out_of_range, DegreeTable, MelodyControls, MAX_DEGREES};

/// Empty degree slot; written degrees are clamped to ±127.
const NO_DEGREE: i32 = i32::MIN;
/// Empty weight slot (a NaN bit pattern no clamped weight is stored as).
const NO_WEIGHT: u32 = u32::MAX;
/// Empty count slot; written counts are at least 1.
const NO_COUNT: usize = 0;

fn weight_bits(value: f32) -> u32 {
    match value.to_bits() {
        NO_WEIGHT => f32::NAN.to_bits(),
        bits => bits,
    }
}

/// Lock-free mailbox for writes that found the degree table busy, plus a
/// lock-free mirror of the effective table for reads that find it busy.
///
/// Allocated once, with room for [`MAX_DEGREES`] positions, so depositing,
/// draining and reading never allocate.
pub(super) struct Pending {
    /// Set after any slot is filled; cleared by a drain before it reads slots.
    any: AtomicBool,
    count: AtomicUsize,
    degrees: [AtomicI32; MAX_DEGREES],
    weights: [AtomicU32; MAX_DEGREES],
    /// The table's active count, as of its last edit.
    shown_count: AtomicUsize,
    /// The degrees and weights (`f32` bits) the active positions play.
    shown_degrees: [AtomicI32; MAX_DEGREES],
    shown_weights: [AtomicU32; MAX_DEGREES],
}

impl Pending {
    /// An empty mailbox mirroring `table`.
    pub(super) fn new(table: &DegreeTable) -> Self {
        let pending = Self {
            any: AtomicBool::new(false),
            count: AtomicUsize::new(NO_COUNT),
            degrees: std::array::from_fn(|_| AtomicI32::new(NO_DEGREE)),
            weights: std::array::from_fn(|_| AtomicU32::new(NO_WEIGHT)),
            shown_count: AtomicUsize::new(0),
            shown_degrees: std::array::from_fn(|_| AtomicI32::new(0)),
            shown_weights: std::array::from_fn(|_| AtomicU32::new(1.0f32.to_bits())),
        };
        pending.publish(table);
        pending
    }

    /// Whether a deposit may be waiting to be drained.
    pub(super) fn has_pending(&self) -> bool {
        self.any.load(Ordering::SeqCst)
    }

    /// The latest written count: a pending one, else the table's.
    pub(super) fn latest_count(&self) -> usize {
        match self.count.load(Ordering::Acquire) {
            NO_COUNT => self.shown_count.load(Ordering::Acquire),
            count => count,
        }
    }

    /// Rejects an index past the latest written count, as the setters do.
    pub(super) fn check_index(&self, what: &str, index: usize) -> Result<(), String> {
        let count = self.latest_count();
        if index < count {
            Ok(())
        } else {
            Err(out_of_range(what, index, count))
        }
    }

    /// The degree at `index` without the lock: pending, else as shown.
    pub(super) fn degree(&self, index: usize) -> Result<i32, String> {
        self.check_index("Degree", index)?;
        Ok(match self.degrees[index].load(Ordering::Acquire) {
            NO_DEGREE => self.shown_degrees[index].load(Ordering::Acquire),
            degree => degree,
        })
    }

    /// The weight at `index` without the lock: pending, else as shown.
    pub(super) fn weight(&self, index: usize) -> Result<f32, String> {
        self.check_index("Weight", index)?;
        Ok(f32::from_bits(
            match self.weights[index].load(Ordering::Acquire) {
                NO_WEIGHT => self.shown_weights[index].load(Ordering::Acquire),
                bits => bits,
            },
        ))
    }

    /// Deposits a degree for a position the caller has validated.
    pub(super) fn deposit_degree(&self, index: usize, value: i32) {
        self.degrees[index].store(value, Ordering::Release);
        self.mark();
    }

    /// Deposits a weight for a position the caller has validated.
    pub(super) fn deposit_weight(&self, index: usize, value: f32) {
        self.weights[index].store(weight_bits(value), Ordering::Release);
        self.mark();
    }

    /// Deposits a count (1 to [`MAX_DEGREES`]).
    pub(super) fn deposit_count(&self, count: usize) {
        self.count.store(count, Ordering::Release);
        self.mark();
    }

    /// Flags a deposit, after its slot is stored. The fence orders the flag
    /// before the depositor's retry of the lock (see the module docs).
    fn mark(&self) {
        self.any.store(true, Ordering::SeqCst);
        fence(Ordering::SeqCst);
    }

    /// Refreshes the mirror of every active position and the count. Call
    /// under the lock after any edit that recomputes the table.
    pub(super) fn publish(&self, table: &DegreeTable) {
        for (i, (&degree, &weight)) in table.degrees.iter().zip(&table.weights).enumerate() {
            self.shown_degrees[i].store(degree, Ordering::Release);
            self.shown_weights[i].store(weight.to_bits(), Ordering::Release);
        }
        self.shown_count.store(table.count(), Ordering::Release);
    }

    /// Mirrors a degree written to one position. Call under the lock.
    pub(super) fn publish_degree(&self, index: usize, value: i32) {
        self.shown_degrees[index].store(value, Ordering::Release);
    }

    /// Mirrors a weight written to one position. Call under the lock.
    pub(super) fn publish_weight(&self, index: usize, value: f32) {
        self.shown_weights[index].store(value.to_bits(), Ordering::Release);
    }

    /// Applies every deposit to `table`: the count first, then positions.
    /// A position past the count by then keeps its value hidden, as if it
    /// was written before the count shrank. Call under the lock. Returns
    /// whether anything was applied. Never allocates.
    ///
    /// The flag is cleared before slots are read, so a deposit racing the
    /// drain sets it again and is caught by the holder's post-release check.
    /// A slot is cleared only if it still holds the value applied, so a newer
    /// deposit to it survives; the mirror is refreshed before the slot is
    /// cleared, so a contended read never sees neither.
    fn drain(&self, table: &mut DegreeTable) -> bool {
        if !self.any.load(Ordering::SeqCst) || !self.any.swap(false, Ordering::SeqCst) {
            return false;
        }
        let mut changed = false;
        let count = self.count.load(Ordering::Acquire);
        if count != NO_COUNT {
            if table.set_count(count) {
                self.publish(table);
                changed = true;
            }
            let _ =
                self.count
                    .compare_exchange(count, NO_COUNT, Ordering::AcqRel, Ordering::Relaxed);
        }
        for i in 0..MAX_DEGREES {
            let degree = self.degrees[i].load(Ordering::Acquire);
            if degree != NO_DEGREE {
                if table.write_degree(i, degree) {
                    self.publish_degree(i, degree);
                }
                changed = true;
                let _ = self.degrees[i].compare_exchange(
                    degree,
                    NO_DEGREE,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                );
            }
            let bits = self.weights[i].load(Ordering::Acquire);
            if bits != NO_WEIGHT {
                let weight = f32::from_bits(bits);
                if table.write_weight(i, weight) {
                    self.publish_weight(i, weight);
                }
                changed = true;
                let _ = self.weights[i].compare_exchange(
                    bits,
                    NO_WEIGHT,
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                );
            }
        }
        changed
    }
}

/// The degree table, locked. Every path that locks the table goes through
/// this guard: it drains the mailbox on acquire and, after releasing, drains
/// deposits that arrived while it held the lock (see the module docs).
pub(super) struct TableGuard<'a> {
    ctrl: &'a MelodyControls,
    table: Option<MutexGuard<'a, DegreeTable>>,
}

impl<'a> TableGuard<'a> {
    fn acquired(ctrl: &'a MelodyControls, mut table: MutexGuard<'a, DegreeTable>) -> Self {
        ctrl.drain_into(&mut table);
        Self {
            ctrl,
            table: Some(table),
        }
    }
}

impl Deref for TableGuard<'_> {
    type Target = DegreeTable;

    fn deref(&self) -> &DegreeTable {
        self.table.as_ref().expect("held until drop")
    }
}

impl DerefMut for TableGuard<'_> {
    fn deref_mut(&mut self) -> &mut DegreeTable {
        self.table.as_mut().expect("held until drop")
    }
}

impl Drop for TableGuard<'_> {
    fn drop(&mut self) {
        self.table = None;
        self.ctrl.drain_released();
    }
}

impl MelodyControls {
    /// Locks the table, blocking. Control thread only.
    pub(super) fn lock_table(&self) -> TableGuard<'_> {
        TableGuard::acquired(self, self.table.lock().unwrap())
    }

    /// Locks the table if it is free; never blocks.
    pub(super) fn try_lock_table(&self) -> Option<TableGuard<'_>> {
        match self.table.try_lock() {
            Ok(table) => Some(TableGuard::acquired(self, table)),
            Err(TryLockError::Poisoned(poisoned)) => {
                Some(TableGuard::acquired(self, poisoned.into_inner()))
            }
            Err(TryLockError::WouldBlock) => None,
        }
    }

    /// Whether a deferred write may be waiting to be drained.
    pub(super) fn has_pending(&self) -> bool {
        self.pending.has_pending()
    }

    /// Drains the mailbox into the locked table, publishing any change.
    fn drain_into(&self, table: &mut DegreeTable) {
        if self.pending.drain(table) {
            self.table_version.fetch_add(1, Ordering::Release);
        }
    }

    /// After releasing the lock: drains deposits made while it was held,
    /// for as long as the lock is free and deposits keep arriving.
    fn drain_released(&self) {
        loop {
            fence(Ordering::SeqCst);
            if !self.pending.has_pending() {
                return;
            }
            let mut table = match self.table.try_lock() {
                Ok(table) => table,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => return,
            };
            self.drain_into(&mut table);
        }
    }

    /// After a deposit: lands it now if the lock has come free.
    pub(super) fn retry_drain(&self) {
        drop(self.try_lock_table());
    }
}

/// Single-position and count controls: none of them ever blocks.
impl MelodyControls {
    /// Gets the number of active degrees. Never blocks.
    pub fn degree_count(&self) -> usize {
        match self.try_lock_table() {
            Some(table) => table.count(),
            None => self.pending.latest_count(),
        }
    }

    /// Sets the number of active degrees (1 to [`MAX_DEGREES`]). Never
    /// blocks: a write that finds the table busy is deferred.
    ///
    /// Shrinking hides the positions past the count; growing shows them again
    /// as they were. Positions past the base scale repeat it from the start.
    pub fn set_degree_count(&self, count: usize) {
        let count = count.clamp(1, MAX_DEGREES);
        match self.try_lock_table() {
            // A ramp writes the count every sample; only a change is an edit.
            Some(mut table) => {
                if table.set_count(count) {
                    self.pending.publish(&table);
                    self.table_version.fetch_add(1, Ordering::Release);
                }
            }
            None => {
                if self.pending.latest_count() != count {
                    self.pending.deposit_count(count);
                    self.retry_drain();
                }
            }
        }
    }

    /// Gets the scale degree at active position `index`. Never blocks.
    pub fn degree(&self, index: usize) -> Result<i32, String> {
        match self.try_lock_table() {
            Some(table) => table
                .degrees
                .get(index)
                .copied()
                .ok_or_else(|| out_of_range("Degree", index, table.count())),
            None => self.pending.degree(index),
        }
    }

    /// Sets the scale degree at active position `index` (clamped to ±127).
    /// Never blocks: a write that finds the table busy is deferred.
    pub fn set_degree(&self, index: usize, value: i32) -> Result<(), String> {
        let value = value.clamp(-127, 127);
        let Some(mut table) = self.try_lock_table() else {
            self.pending.check_index("Degree", index)?;
            self.pending.deposit_degree(index, value);
            self.retry_drain();
            return Ok(());
        };
        if index >= table.count() {
            return Err(out_of_range("Degree", index, table.count()));
        }
        table.write_degree(index, value);
        self.pending.publish_degree(index, value);
        self.table_version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Gets the note weight at active position `index`. Never blocks.
    pub fn note_weight(&self, index: usize) -> Result<f32, String> {
        match self.try_lock_table() {
            Some(table) => table
                .weights
                .get(index)
                .copied()
                .ok_or_else(|| out_of_range("Weight", index, table.count())),
            None => self.pending.weight(index),
        }
    }

    /// Sets the note weight at active position `index` (clamped to 0-10).
    /// Never blocks: a write that finds the table busy is deferred.
    pub fn set_note_weight(&self, index: usize, value: f32) -> Result<(), String> {
        let value = value.clamp(0.0, 10.0);
        let Some(mut table) = self.try_lock_table() else {
            self.pending.check_index("Weight", index)?;
            self.pending.deposit_weight(index, value);
            self.retry_drain();
            return Ok(());
        };
        if index >= table.count() {
            return Err(out_of_range("Weight", index, table.count()));
        }
        table.write_weight(index, value);
        self.pending.publish_weight(index, value);
        self.table_version.fetch_add(1, Ordering::Release);
        Ok(())
    }
}
