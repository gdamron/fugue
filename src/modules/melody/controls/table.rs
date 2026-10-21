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
//!
//! Setting a whole scale or whole weights forgets the values written to
//! single positions by moving on a generation, not by clearing them: a
//! written value counts only while it carries the current generation. The
//! count and the degrees' generation share one atomic, so a position write
//! checked against one count is tagged with that count's generation. A
//! write racing a whole scale is then either before it (forgotten with the
//! rest) or after it (kept), as under a lock, never accepted and then lost.

use std::sync::atomic::{fence, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::audio_thread::debug_assert_control_thread;

/// Maximum number of scale degrees a melody can choose between.
///
/// Bounds the table and the audio thread's copy of it.
pub(crate) const MAX_DEGREES: usize = 128;

/// How many times a single read retries an edit of many entries overlapping
/// it before taking what it read.
const READ_ATTEMPTS: usize = 4;

/// The active count, from a header (low 32 bits).
fn count_of(header: u64) -> usize {
    header as u32 as usize
}

/// The degrees' generation, from a header (high 32 bits).
fn generation_of(header: u64) -> u32 {
    (header >> 32) as u32
}

fn header(generation: u32, count: usize) -> u64 {
    u64::from(generation) << 32 | count as u64
}

/// The generation after `generation`, skipping 0, which marks a position
/// never written.
fn next_generation(generation: u32) -> u32 {
    generation.checked_add(1).unwrap_or(1)
}

/// The value written to one position in `generation`, if any.
fn written(slot: &AtomicU64, generation: u32) -> Option<u32> {
    let bits = slot.load(Ordering::Acquire);
    (generation_of(bits) == generation).then_some(bits as u32)
}

fn write(slot: &AtomicU64, generation: u32, bits: u32) {
    slot.store(
        u64::from(generation) << 32 | u64::from(bits),
        Ordering::Release,
    );
}

/// Scale degrees and their selection weights (see the module docs).
pub(crate) struct DegreeTable {
    /// The number of active positions and the degrees' generation, packed
    /// (see [`header`]).
    header: AtomicU64,
    /// The base scale, as configured or set whole: `scale_len` of them.
    scale: [AtomicI32; MAX_DEGREES],
    scale_len: AtomicU32,
    /// The base weights (`f32` bits), as configured or set whole:
    /// `weights_len` of them, which may be fewer or more than the scale's.
    scale_weights: [AtomicU32; MAX_DEGREES],
    weights_len: AtomicU32,
    /// The weights' generation.
    weights_generation: AtomicU32,
    /// Degrees written to single positions, each tagged with its
    /// generation, kept while hidden.
    written_degrees: [AtomicU64; MAX_DEGREES],
    /// Weights (`f32` bits) written to single positions, likewise.
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
            header: AtomicU64::new(header(1, scale.len())),
            scale: std::array::from_fn(|i| AtomicI32::new(scale.get(i).copied().unwrap_or(0))),
            scale_len: AtomicU32::new(scale.len() as u32),
            scale_weights: std::array::from_fn(|_| AtomicU32::new(0)),
            weights_len: AtomicU32::new(0),
            weights_generation: AtomicU32::new(1),
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
        count_of(self.header.load(Ordering::Acquire))
    }

    /// The degree position `i` plays, with the degrees in `generation`.
    fn degree_in(&self, generation: u32, i: usize) -> i32 {
        if let Some(bits) = written(&self.written_degrees[i], generation) {
            return bits as i32;
        }
        match self.scale_len.load(Ordering::Acquire) as usize {
            0 => 0,
            n => self.scale[i % n].load(Ordering::Relaxed),
        }
    }

    /// The weight position `i` takes, with the weights in `generation`.
    fn weight_in(&self, generation: u32, i: usize) -> f32 {
        if let Some(bits) = written(&self.written_weights[i], generation) {
            return f32::from_bits(bits);
        }
        let given = self.weights_len.load(Ordering::Acquire) as usize;
        let n = self.scale_len.load(Ordering::Acquire) as usize;
        let at = match (i < given, n) {
            (true, _) => i,
            (false, 0) => return 1.0,
            (false, n) if i % n < given => i % n,
            _ => return 1.0,
        };
        f32::from_bits(self.scale_weights[at].load(Ordering::Relaxed))
    }

    /// Calls `read` with the header and the weights' generation, and
    /// returns what it read once no edit of many entries overlapped it:
    /// `None` if one did, every time of `attempts`.
    fn consistent<R>(&self, attempts: usize, mut read: impl FnMut(u64, u32) -> R) -> Option<R> {
        for _ in 0..attempts {
            let sequence = self.sequence.load(Ordering::Acquire);
            if sequence % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let header = self.header.load(Ordering::Acquire);
            let weights = self.weights_generation.load(Ordering::Acquire);
            let value = read(header, weights);
            fence(Ordering::Acquire);
            if self.sequence.load(Ordering::Relaxed) == sequence {
                return Some(value);
            }
        }
        None
    }

    /// The degree and weight at active position `index`, or `None` past the
    /// count. Any thread, never waiting: an edit of many entries that keeps
    /// overlapping the read leaves it with the positions as they read
    /// mid-edit, each a value the table held or is taking.
    pub(crate) fn position(&self, index: usize) -> Option<(i32, f32)> {
        let read = |header: u64, weights: u32| {
            (index < count_of(header)).then(|| {
                let degree = self.degree_in(generation_of(header), index);
                (degree, self.weight_in(weights, index))
            })
        };
        match self.consistent(READ_ATTEMPTS, &read) {
            Some(position) => position,
            None => {
                let weights = self.weights_generation.load(Ordering::Acquire);
                read(self.header.load(Ordering::Acquire), weights)
            }
        }
    }

    /// The active degrees, in order. Control thread.
    pub(crate) fn degrees(&self) -> Vec<i32> {
        let _whole = self.whole();
        let generation = generation_of(self.header.load(Ordering::Acquire));
        (0..self.count())
            .map(|i| self.degree_in(generation, i))
            .collect()
    }

    /// The active weights, in order. Control thread.
    pub(crate) fn weights(&self) -> Vec<f32> {
        let _whole = self.whole();
        let generation = self.weights_generation.load(Ordering::Acquire);
        (0..self.count())
            .map(|i| self.weight_in(generation, i))
            .collect()
    }

    /// Sets the number of active positions (clamped to 1 to
    /// [`MAX_DEGREES`]), returning whether it changed. Any thread.
    pub(crate) fn set_count(&self, count: usize) -> bool {
        let count = count.clamp(1, MAX_DEGREES);
        let previous = self
            .header
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |was| {
                Some(header(generation_of(was), count))
            });
        let changed = previous.is_ok_and(|was| count_of(was) != count);
        if changed {
            self.changed();
        }
        changed
    }

    /// Writes the degree at active position `index` (clamped to ±127),
    /// returning false for a position past the count. Any thread.
    pub(crate) fn write_degree(&self, index: usize, value: i32) -> bool {
        self.write_degree_as_of(self.header.load(Ordering::Acquire), index, value)
    }

    /// [`Self::write_degree`], checked against and tagged with `header`.
    fn write_degree_as_of(&self, header: u64, index: usize, value: i32) -> bool {
        if index >= count_of(header) {
            return false;
        }
        let bits = value.clamp(-127, 127) as u32;
        write(&self.written_degrees[index], generation_of(header), bits);
        self.changed();
        true
    }

    /// Writes the weight at active position `index` (clamped to 0-10),
    /// returning false for a position past the count. Any thread.
    pub(crate) fn write_weight(&self, index: usize, value: f32) -> bool {
        let generation = self.weights_generation.load(Ordering::Acquire);
        if index >= self.count() {
            return false;
        }
        let bits = value.clamp(0.0, 10.0).to_bits();
        write(&self.written_weights[index], generation, bits);
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
            table
                .scale_len
                .store(degrees.len() as u32, Ordering::Release);
            // Under the edit lock no other edit moves the generation; a
            // count set meanwhile is set before this scale.
            let generation = generation_of(table.header.load(Ordering::Acquire));
            let next = header(next_generation(generation), degrees.len());
            table.header.store(next, Ordering::Release);
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
            table
                .weights_len
                .store(weights.len() as u32, Ordering::Release);
            let generation = table.weights_generation.load(Ordering::Acquire);
            (table.weights_generation).store(next_generation(generation), Ordering::Release);
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
            let generation = generation_of(table.header.load(Ordering::Acquire));
            for (index, value) in degrees {
                if index < MAX_DEGREES {
                    let bits = value.clamp(-127, 127) as u32;
                    write(&table.written_degrees[index], generation, bits);
                }
            }
            let generation = table.weights_generation.load(Ordering::Acquire);
            for (index, value) in weights {
                if index < MAX_DEGREES {
                    let bits = value.clamp(0.0, 10.0).to_bits();
                    write(&table.written_weights[index], generation, bits);
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
        self.consistent(1, |header, generation| {
            let count = count_of(header).min(MAX_DEGREES);
            for i in 0..count {
                degrees[i] = self.degree_in(generation_of(header), i);
                weights[i] = self.weight_in(generation, i);
            }
            count
        })
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
