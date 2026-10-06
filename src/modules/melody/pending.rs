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
//! Every holder of the table lock goes through [`TableGuard`] or, like the
//! guard's own post-release drain, drains the mailbox itself. The guard
//! drains when it acquires the lock, so a direct write lands after older
//! deferred ones, and a whole-table edit overwrites them rather than being
//! overwritten.
//!
//! Invariant: a deposit is never stranded. The argument rests on the memory
//! model alone, not on how `Mutex` is built:
//!
//! - (a) Publication. A depositor stores its slot and then flags the deposit
//!   with an RMW, `any.swap(true, AcqRel)`. Every write to `any` is an RMW,
//!   so each continues the release sequence of every earlier one. A
//!   drainer's `any.swap(false, AcqRel)` that reads `true` therefore
//!   synchronizes with every depositor whose RMW precedes it in `any`'s
//!   modification order, and sees their slots. A deposit whose RMW comes
//!   later leaves the flag set.
//! - (b) Eventual drain. A set flag stays set until a drainer clears it,
//!   under the lock, and that drainer applies the slot (by (a)).
//!   `MelodyGenerator::process` runs `DegreeSnapshot::sync` every block,
//!   which drains with `try_lock` whenever the flag is set. A scheduler
//!   deposits on that same audio thread, so its deposits are visible to the
//!   next sync in program order; a control thread's become visible in
//!   finite time. Contention is transient, so a later block's `try_lock`
//!   succeeds. Every lock holder also drains on acquire, and contended reads
//!   consult the mailbox, so a pending write is never read past.
//! - (c) The depositor's `try_lock` retry and the holder's post-release
//!   re-check (at most `MAX_RELEASE_DRAINS` passes) only cut latency; the
//!   argument above does not depend on them.
//!
//! # Generations
//!
//! A deferred write carries the generation of its field (the scale for a
//! degree, the base weights for a weight) that it was validated against.
//! One that a replacement of its field overtook (`set_allowed_degrees`,
//! `set_note_weights`) is ordered before that replacement, which erased it,
//! and is dropped; otherwise it applies, kept hidden if its position is
//! past the count by then (count changes keep written values).
//!
//! A replacement bumps its field's generation with Release only after it has
//! published the new count and values to the mirror, and a depositor loads
//! the generation with Acquire before it validates against the count. So a
//! deposit tagged with the new generation was validated against the new
//! count; one validated against the old count carries the old generation
//! and is dropped whatever it was validated against.
//!
//! A deposit never overwrites one tagged with a newer, current generation:
//! that write came after the replacement that overtook ours, so ours is
//! ordered before both and dropped. Any other deposit in the slot (none, the
//! same generation, or a stale one) is overwritten, so the last write wins.
//!
//! # Reads while contended
//!
//! Reads that find the table busy answer from the mailbox (the pending value
//! for that slot, if its generation is current) or else from a lock-free
//! mirror of the effective table, refreshed under the lock after every edit.
//!
//! # Known gaps
//!
//! - A position revealed by a count growth that is still pending reads its
//!   mirror while the table is busy, which may be stale (it holds whatever
//!   the position last played, or 0) until the growth is drained.
//! - Generations wrap after 2^32 - 1 replacements of one field; a deposit
//!   left pending across exactly that many would be taken as current.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{MutexGuard, TryLockError};

use super::controls::{out_of_range, DegreeTable, MelodyControls, MAX_DEGREES};

/// Empty position slot. Its generation half, `u32::MAX`, is never a live
/// generation (see [`Pending::bump`]), so no deposit packs to it.
const EMPTY: u64 = u64::MAX;
/// Empty count slot; written counts are at least 1.
const NO_COUNT: usize = 0;
/// Passes a post-release drain makes before leaving a deposit that keeps
/// racing it to the next holder or the melody's next block.
const MAX_RELEASE_DRAINS: usize = 4;

/// Packs a deposit: its field's generation, then the value's bits.
fn pack(generation: u32, bits: u32) -> u64 {
    (u64::from(generation) << 32) | u64::from(bits)
}

/// The value bits of a deposit packed with `generation`, if it was.
fn live(packed: u64, generation: u32) -> Option<u32> {
    ((packed >> 32) as u32 == generation).then_some(packed as u32)
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
    /// Deferred degrees (`i32` bits) and weights (`f32` bits), each packed
    /// with the generation it was validated against, or [`EMPTY`].
    degrees: [AtomicU64; MAX_DEGREES],
    weights: [AtomicU64; MAX_DEGREES],
    /// Bumped by each replacement of the scale, and of the base weights.
    degrees_gen: AtomicU32,
    weights_gen: AtomicU32,
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
            degrees: std::array::from_fn(|_| AtomicU64::new(EMPTY)),
            weights: std::array::from_fn(|_| AtomicU64::new(EMPTY)),
            degrees_gen: AtomicU32::new(0),
            weights_gen: AtomicU32::new(0),
            shown_count: AtomicUsize::new(0),
            shown_degrees: std::array::from_fn(|_| AtomicI32::new(0)),
            shown_weights: std::array::from_fn(|_| AtomicU32::new(1.0f32.to_bits())),
        };
        pending.publish(table);
        pending
    }

    /// Whether a deposit may be waiting to be drained.
    pub(super) fn has_pending(&self) -> bool {
        self.any.load(Ordering::Acquire)
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
        let packed = self.degrees[index].load(Ordering::Acquire);
        Ok(match live(packed, self.degrees_gen()) {
            Some(bits) => bits as i32,
            None => self.shown_degrees[index].load(Ordering::Acquire),
        })
    }

    /// The weight at `index` without the lock: pending, else as shown.
    pub(super) fn weight(&self, index: usize) -> Result<f32, String> {
        self.check_index("Weight", index)?;
        let packed = self.weights[index].load(Ordering::Acquire);
        Ok(f32::from_bits(match live(packed, self.weights_gen()) {
            Some(bits) => bits,
            None => self.shown_weights[index].load(Ordering::Acquire),
        }))
    }

    /// The scale's generation. Load it before validating a deferred degree.
    pub(super) fn degrees_gen(&self) -> u32 {
        self.degrees_gen.load(Ordering::Acquire)
    }

    /// The base weights' generation. Load it before validating a weight.
    pub(super) fn weights_gen(&self) -> u32 {
        self.weights_gen.load(Ordering::Acquire)
    }

    /// Deposits a degree for a position validated under `generation`.
    pub(super) fn deposit_degree(&self, generation: u32, index: usize, value: i32) {
        let slot = &self.degrees[index];
        self.deposit(slot, &self.degrees_gen, generation, value as u32);
    }

    /// Deposits a weight for a position validated under `generation`.
    pub(super) fn deposit_weight(&self, generation: u32, index: usize, value: f32) {
        let slot = &self.weights[index];
        self.deposit(slot, &self.weights_gen, generation, value.to_bits());
    }

    /// Stores a deposit in `slot` and flags it, unless the slot holds one
    /// tagged with a newer, current generation of its field (see
    /// "Generations"); then ours is dropped, unflagged.
    ///
    /// The CAS loop is lock-free, not wait-free: each retry means another
    /// writer changed this slot, so some thread always makes progress, none
    /// ever blocks, and retries are bounded in practice by the writers
    /// racing on this one position. That suits the audio thread; a give-up
    /// path could instead drop a current write.
    fn deposit(&self, slot: &AtomicU64, field_gen: &AtomicU32, generation: u32, bits: u32) {
        let packed = pack(generation, bits);
        let mut current = slot.load(Ordering::Acquire);
        loop {
            let held = (current >> 32) as u32;
            if current != EMPTY && held != generation && held == field_gen.load(Ordering::Acquire) {
                return;
            }
            match slot.compare_exchange_weak(current, packed, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        self.mark();
    }

    /// Deposits a count (1 to [`MAX_DEGREES`]).
    pub(super) fn deposit_count(&self, count: usize) {
        self.count.store(count, Ordering::Release);
        self.mark();
    }

    /// Flags a deposit, after its slot is stored. An RMW, not a store, so
    /// it continues earlier depositors' release sequences (see the module
    /// docs).
    fn mark(&self) {
        self.any.swap(true, Ordering::AcqRel);
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

    /// Starts a new generation of the scale (`degrees`) or base weights.
    /// Call under the lock (the only writer) after publishing the edit.
    /// Skips `u32::MAX`, which marks an empty slot.
    pub(super) fn bump(&self, degrees: bool) {
        let generation = if degrees {
            &self.degrees_gen
        } else {
            &self.weights_gen
        };
        let next = generation.load(Ordering::Relaxed).wrapping_add(1);
        generation.store(if next == u32::MAX { 0 } else { next }, Ordering::Release);
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
    /// A position deposit whose generation is stale is dropped; a current
    /// one applies, kept hidden if past the count (see "Generations"). Call
    /// under the lock. Returns whether anything was applied. Never
    /// allocates.
    ///
    /// The flag is cleared before slots are read, so a deposit racing the
    /// drain sets it again and is drained later (see the module docs).
    /// A slot is cleared only if it still holds the deposit read, so a newer
    /// deposit to it survives; the mirror is refreshed before the slot is
    /// cleared, so a contended read never sees neither.
    fn drain(&self, table: &mut DegreeTable) -> bool {
        // Either exit is sound. A `false` load reads the value some clearing
        // swap wrote (or the initial one): that drainer applied every deposit
        // flagged before it, and any flagged after leaves the flag set for a
        // later drain. A `false` swap means another holder cleared the flag
        // and, since we hold the lock now, has finished applying.
        if !self.any.load(Ordering::Acquire) || !self.any.swap(false, Ordering::AcqRel) {
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
        let (degrees_gen, weights_gen) = (self.degrees_gen(), self.weights_gen());
        for i in 0..MAX_DEGREES {
            let packed = self.degrees[i].load(Ordering::Acquire);
            if packed != EMPTY {
                if let Some(bits) = live(packed, degrees_gen) {
                    if table.write_degree(i, bits as i32) {
                        self.publish_degree(i, bits as i32);
                    }
                    changed = true;
                }
                clear(&self.degrees[i], packed);
            }
            let packed = self.weights[i].load(Ordering::Acquire);
            if packed != EMPTY {
                if let Some(bits) = live(packed, weights_gen) {
                    let weight = f32::from_bits(bits);
                    if table.write_weight(i, weight) {
                        self.publish_weight(i, weight);
                    }
                    changed = true;
                }
                clear(&self.weights[i], packed);
            }
        }
        changed
    }
}

/// Empties `slot` if it still holds the deposit `packed`.
fn clear(slot: &AtomicU64, packed: u64) {
    let _ = slot.compare_exchange(packed, EMPTY, Ordering::AcqRel, Ordering::Relaxed);
}

/// The degree table, locked. Every path that locks the table goes through
/// this guard or, like `MelodyControls::drain_released`, repeats its
/// re-check itself: it drains the mailbox on acquire and, after releasing,
/// drains deposits that arrived while it held the lock (see the module
/// docs).
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
    /// while the lock is free and deposits keep arriving, for at most
    /// `MAX_RELEASE_DRAINS` passes. Stopping with a deposit still pending
    /// is safe: the melody's `DegreeSnapshot::sync` drains it on its next
    /// block, and any later holder drains it on acquire.
    fn drain_released(&self) {
        for _ in 0..MAX_RELEASE_DRAINS {
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
