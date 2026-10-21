//! The melody's degree table, in atomics: any thread reads it, and a
//! scheduler writes one position or the count on the audio thread with a
//! store, never a lock.
//!
//! What the melody plays at position `i` is computed from four sources: the
//! base `scale` and `scale_weights`, the active `count`, and the values
//! written to single positions. So the same sources always give the same
//! table, whatever order they were set in, and a document that records them
//! rebuilds exactly what was playing.
//!
//! - Shrinking the count hides the positions past it; growing it again shows
//!   them as they were, written values included.
//! - Growing past the scale repeats it: position `i` plays `scale[i % n]`
//!   with weight `scale_weights[i % n]`, unless a weight is given for `i`.
//!   An empty scale grows with degree 0.
//!
//! A write to one position or to the count is one store and a version bump,
//! from any thread. Edits of many entries at once (a whole scale, whole
//! weights, a document's written values) run on control threads only, one
//! at a time, and mark themselves in a sequence count that is odd while
//! they write: the audio thread's copy ([`super::super::snapshot`]) is taken
//! only when it was even and unchanged across the copy, so it never holds
//! half of one scale and half of another.

use std::sync::atomic::{fence, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::audio_thread::debug_assert_control_thread;

/// Maximum number of scale degrees a melody can choose between.
///
/// Bounds the table and the audio thread's copy of it.
pub(crate) const MAX_DEGREES: usize = 128;

/// A written value's flag: the value is in the low 32 bits.
const WRITTEN: u64 = 1 << 32;

/// The value written to one position, if any.
fn written(slot: &AtomicU64) -> Option<u32> {
    let bits = slot.load(Ordering::Acquire);
    (bits & WRITTEN != 0).then_some(bits as u32)
}

fn write(slot: &AtomicU64, bits: u32) {
    slot.store(WRITTEN | u64::from(bits), Ordering::Release);
}

/// Scale degrees and their selection weights (see the module docs).
pub(crate) struct DegreeTable {
    /// Number of active positions.
    count: AtomicUsize,
    /// The base scale, as configured or set whole: `scale_len` of them.
    scale: [AtomicI32; MAX_DEGREES],
    scale_len: AtomicUsize,
    /// The base weights (`f32` bits), as configured or set whole:
    /// `weights_len` of them, which may be fewer or more than the scale's.
    scale_weights: [AtomicU32; MAX_DEGREES],
    weights_len: AtomicUsize,
    /// Degrees written to single positions, kept while hidden.
    written_degrees: [AtomicU64; MAX_DEGREES],
    /// Weights (`f32` bits) written to single positions, kept while hidden.
    written_weights: [AtomicU64; MAX_DEGREES],
    /// Bumped after every change; the audio thread re-copies the table when
    /// it observes one.
    version: AtomicU64,
    /// Odd while an edit of many entries writes.
    sequence: AtomicU64,
    /// Control threads only: one edit of many entries at a time, and
    /// readers that want the table whole.
    editing: Mutex<()>,
}

impl DegreeTable {
    pub(super) fn new(mut scale: Vec<i32>) -> Self {
        scale.truncate(MAX_DEGREES);
        Self {
            count: AtomicUsize::new(scale.len()),
            scale: std::array::from_fn(|i| AtomicI32::new(scale.get(i).copied().unwrap_or(0))),
            scale_len: AtomicUsize::new(scale.len()),
            scale_weights: std::array::from_fn(|_| AtomicU32::new(0)),
            weights_len: AtomicUsize::new(0),
            written_degrees: std::array::from_fn(|_| AtomicU64::new(0)),
            written_weights: std::array::from_fn(|_| AtomicU64::new(0)),
            version: AtomicU64::new(0),
            sequence: AtomicU64::new(0),
            editing: Mutex::new(()),
        }
    }

    /// Monotonic change counter.
    pub(crate) fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn changed(&self) {
        self.version.fetch_add(1, Ordering::Release);
    }

    /// Number of active positions.
    pub(crate) fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    /// The degree position `i` plays.
    pub(crate) fn degree_at(&self, i: usize) -> i32 {
        if let Some(bits) = written(&self.written_degrees[i]) {
            return bits as i32;
        }
        match self.scale_len.load(Ordering::Acquire) {
            0 => 0,
            n => self.scale[i % n].load(Ordering::Relaxed),
        }
    }

    /// The weight position `i` takes.
    pub(crate) fn weight_at(&self, i: usize) -> f32 {
        if let Some(bits) = written(&self.written_weights[i]) {
            return f32::from_bits(bits);
        }
        let given = self.weights_len.load(Ordering::Acquire);
        let n = self.scale_len.load(Ordering::Acquire);
        let at = match (i < given, n) {
            (true, _) => i,
            (false, 0) => return 1.0,
            (false, n) if i % n < given => i % n,
            _ => return 1.0,
        };
        f32::from_bits(self.scale_weights[at].load(Ordering::Relaxed))
    }

    /// Sets the number of active positions (clamped to 1 to
    /// [`MAX_DEGREES`]), returning whether it changed. Any thread.
    pub(crate) fn set_count(&self, count: usize) -> bool {
        let count = count.clamp(1, MAX_DEGREES);
        let changed = self.count.swap(count, Ordering::AcqRel) != count;
        if changed {
            self.changed();
        }
        changed
    }

    /// Writes the degree at active position `index` (clamped to ±127),
    /// returning false for a position past the count. Any thread.
    pub(crate) fn write_degree(&self, index: usize, value: i32) -> bool {
        if index >= self.count() {
            return false;
        }
        write(&self.written_degrees[index], value.clamp(-127, 127) as u32);
        self.changed();
        true
    }

    /// Writes the weight at active position `index` (clamped to 0-10),
    /// returning false for a position past the count. Any thread.
    pub(crate) fn write_weight(&self, index: usize, value: f32) -> bool {
        if index >= self.count() {
            return false;
        }
        write(
            &self.written_weights[index],
            value.clamp(0.0, 10.0).to_bits(),
        );
        self.changed();
        true
    }

    /// Holds the table whole against edits of many entries. Control thread.
    pub(crate) fn whole(&self) -> MutexGuard<'_, ()> {
        debug_assert_control_thread("reading a melody's whole degree table");
        self.editing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes an edit of many entries, marked odd in the sequence count
    /// while it writes (see the module docs). Control thread.
    fn edit(&self, edit: impl FnOnce(&Self)) {
        let _whole = self.whole();
        self.sequence.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        edit(self);
        self.sequence.fetch_add(1, Ordering::Release);
        self.changed();
    }

    /// Sets the base scale; the count becomes its length, and degrees
    /// written to single positions are forgotten.
    pub(crate) fn set_scale(&self, mut degrees: Vec<i32>) {
        degrees.truncate(MAX_DEGREES);
        self.edit(|table| {
            for (slot, &degree) in table.scale.iter().zip(&degrees) {
                slot.store(degree, Ordering::Relaxed);
            }
            table.scale_len.store(degrees.len(), Ordering::Release);
            table.count.store(degrees.len(), Ordering::Release);
            for slot in &table.written_degrees {
                slot.store(0, Ordering::Release);
            }
        });
    }

    /// Sets the base weights, forgetting weights written to single
    /// positions. Past [`MAX_DEGREES`] no position reads them.
    pub(crate) fn set_scale_weights(&self, mut weights: Vec<f32>) {
        weights.truncate(MAX_DEGREES);
        self.edit(|table| {
            for (slot, weight) in table.scale_weights.iter().zip(&weights) {
                slot.store(weight.to_bits(), Ordering::Relaxed);
            }
            table.weights_len.store(weights.len(), Ordering::Release);
            for slot in &table.written_weights {
                slot.store(0, Ordering::Release);
            }
        });
    }

    /// Restores degrees and weights written to single positions, active or
    /// not.
    pub(crate) fn restore_written(
        &self,
        degrees: impl IntoIterator<Item = (usize, i32)>,
        weights: impl IntoIterator<Item = (usize, f32)>,
    ) {
        self.edit(|table| {
            for (index, value) in degrees {
                if index < MAX_DEGREES {
                    write(&table.written_degrees[index], value.clamp(-127, 127) as u32);
                }
            }
            for (index, value) in weights {
                if index < MAX_DEGREES {
                    write(
                        &table.written_weights[index],
                        value.clamp(0.0, 10.0).to_bits(),
                    );
                }
            }
        });
    }

    /// Copies the active degrees and weights into `degrees` and `weights`,
    /// returning how many, or `None` when an edit of many entries wrote
    /// during the copy: keep the copy before. Never waits.
    pub(crate) fn read_into(
        &self,
        degrees: &mut [i32; MAX_DEGREES],
        weights: &mut [f32; MAX_DEGREES],
    ) -> Option<usize> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence % 2 == 1 {
            return None;
        }
        let count = self.count().min(MAX_DEGREES);
        for i in 0..count {
            degrees[i] = self.degree_at(i);
            weights[i] = self.weight_at(i);
        }
        fence(Ordering::Acquire);
        (self.sequence.load(Ordering::Relaxed) == sequence).then_some(count)
    }

    /// Marks an edit of many entries as writing until the guard is
    /// dropped, for tests of readers that meet one.
    #[cfg(test)]
    pub(crate) fn writing(&self) -> impl Drop + '_ {
        struct Writing<'a> {
            table: &'a DegreeTable,
            _whole: MutexGuard<'a, ()>,
        }
        impl Drop for Writing<'_> {
            fn drop(&mut self) {
                self.table.sequence.fetch_add(1, Ordering::Release);
            }
        }
        let _whole = self.whole();
        self.sequence.fetch_add(1, Ordering::Relaxed);
        Writing {
            table: self,
            _whole,
        }
    }
}

#[cfg(test)]
mod tests;
