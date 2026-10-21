//! The melody's degree table, in atomics: any thread reads it, and a
//! scheduler writes one position or the count on the audio thread without
//! a lock.
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
//! Setting a whole scale or whole weights moves on a generation: a value
//! written to one position counts only while it carries the current
//! generation, so moving on forgets every one of them in one store. The
//! count and the degrees' generation share one atomic, so a position write
//! checked against one count is tagged with that count's generation, and a
//! write never replaces a value tagged with a later generation than its
//! own. A write racing a whole scale is then either before it (forgotten
//! with the rest) or after it (kept), as under a lock.
//!
//! Each generation reads its base from one of two buffers, alternating, so
//! an edit writes the buffer no reader of the current generation reads,
//! and publishes it whole with the generation. A reader re-checks the
//! generations after reading: a change means an edit was published
//! meanwhile, and it reads again. Every publish also bumps a 64-bit
//! counter the reader re-checks, so a read outlasting a whole round of
//! generations is not mistaken for one that saw none. Edits run on control
//! threads, one at a time; a control thread preempted mid-edit holds up no
//! reader. A copy of the whole table is kept only when no change finished
//! during it (each bumps the version after it lands): it is a table that
//! was, never one write and not another that finished before it.
//!
//! Generations are 32 bits, so they come back round. A position write is
//! marked in flight, in the current epoch, from before it reads the
//! generation it checks against, and an edit waits out the writes in flight
//! twice a round, before publishing generation 2 and generation 2^31: it
//! moves on the epoch and waits for the writes marked in the one before,
//! while later writes mark the new one. A write still holding a generation
//! when it is published again would have to outlast one of those waits.

use std::sync::atomic::{fence, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::audio_thread::debug_assert_control_thread;

mod parts;
use parts::*;

/// Maximum number of scale degrees a melody can choose between.
///
/// Bounds the table and the audio thread's copy of it.
pub(crate) const MAX_DEGREES: usize = 128;

/// How many times a read of one position retries before giving up. Each
/// retry means a whole edit was published during the read; an edit takes
/// far longer than a read, so a read retries once at most in practice.
const READ_ATTEMPTS: usize = 64;

/// Scale degrees and their selection weights (see the module docs).
pub(crate) struct DegreeTable {
    /// The number of active positions and the degrees' generation, packed
    /// (see [`header`]).
    header: AtomicU64,
    /// The weights' generation.
    weights_generation: AtomicU32,
    /// The base scale, as configured or set whole, per [`buffer`].
    scales: [Base; 2],
    /// The base weights, as configured or set whole, per [`buffer`]: as
    /// many as given, which may be fewer or more than the scale's.
    weights: [Base; 2],
    /// Degrees written to single positions, each tagged with its
    /// generation, kept while hidden.
    written_degrees: [AtomicU64; MAX_DEGREES],
    /// Weights (`f32` bits) written to single positions, likewise.
    written_weights: [AtomicU64; MAX_DEGREES],
    /// Which of `writes` a position write marks itself in: an edit
    /// waiting out the writes in flight moves it on (see [`Self::drain_before`]).
    epoch: AtomicU64,
    /// Position writes in flight, per epoch parity.
    writes: [AtomicU64; 2],
    /// Bumped after every publish of a generation, so a reader tells a
    /// generation come back round from one that never moved.
    published: AtomicU64,
    /// Bumped after every change; the audio thread re-copies the table when
    /// it observes one.
    version: AtomicU64,
    /// Control threads only: one whole edit at a time.
    editing: Mutex<()>,
}

/// The table as of one degrees' header and one weights' generation.
struct View<'a> {
    table: &'a DegreeTable,
    header: u64,
    weights: u32,
}

impl View<'_> {
    fn count(&self) -> usize {
        count_of(self.header).min(MAX_DEGREES)
    }

    fn scale(&self) -> &Base {
        &self.table.scales[buffer(generation_of(self.header))]
    }

    /// The degree position `i` plays.
    fn degree(&self, i: usize) -> i32 {
        let generation = generation_of(self.header);
        if let Some(bits) = written(&self.table.written_degrees[i], generation) {
            return bits as i32;
        }
        match self.scale().len() {
            0 => 0,
            n => self.scale().get(i % n) as i32,
        }
    }

    /// The weight position `i` takes.
    fn weight(&self, i: usize) -> f32 {
        if let Some(bits) = written(&self.table.written_weights[i], self.weights) {
            return f32::from_bits(bits);
        }
        let given = &self.table.weights[buffer(self.weights)];
        let at = match (i < given.len(), self.scale().len()) {
            (true, _) => i,
            (false, 0) => return 1.0,
            (false, n) if i % n < given.len() => i % n,
            _ => return 1.0,
        };
        f32::from_bits(given.get(at))
    }

    fn position(&self, index: usize) -> Option<(i32, f32)> {
        (index < self.count()).then(|| (self.degree(index), self.weight(index)))
    }
}

impl DegreeTable {
    pub(super) fn new(mut scale: Vec<i32>) -> Self {
        scale.truncate(MAX_DEGREES);
        let bits: Vec<u32> = scale.iter().map(|&degree| degree as u32).collect();
        // Generation 1 reads buffer 1.
        Self {
            header: AtomicU64::new(header(1, scale.len())),
            weights_generation: AtomicU32::new(1),
            scales: [Base::new(&[]), Base::new(&bits)],
            weights: [Base::new(&[]), Base::new(&[])],
            written_degrees: std::array::from_fn(|_| AtomicU64::new(0)),
            written_weights: std::array::from_fn(|_| AtomicU64::new(0)),
            epoch: AtomicU64::new(0),
            writes: [AtomicU64::new(0), AtomicU64::new(0)],
            published: AtomicU64::new(0),
            version: AtomicU64::new(0),
            editing: Mutex::new(()),
        }
    }

    /// Monotonic change counter.
    pub(crate) fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn changed(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }

    /// Number of active positions.
    pub(crate) fn count(&self) -> usize {
        count_of(self.header.load(Ordering::Acquire))
    }

    fn view(&self) -> View<'_> {
        let header = self.header.load(Ordering::Acquire);
        View {
            table: self,
            header,
            weights: self.weights_generation.load(Ordering::Acquire),
        }
    }

    /// Calls `read` with a view of the table, and returns what it read once
    /// nothing was published during it (so the view is the table as it
    /// stood between the two), and, for a `whole` copy, no change finished
    /// during it: `None` if one did, every time of `attempts`. A write in
    /// flight but not finished may or may not be in the copy; it orders
    /// after the copy or before.
    fn consistent<R>(
        &self,
        attempts: usize,
        whole: bool,
        mut read: impl FnMut(&View) -> R,
    ) -> Option<R> {
        for _ in 0..attempts {
            let published = self.published.load(Ordering::SeqCst);
            let version = self.version.load(Ordering::SeqCst);
            let view = self.view();
            let value = read(&view);
            fence(Ordering::Acquire);
            let now = self.view();
            let moved = now.header != view.header
                || now.weights != view.weights
                || self.published.load(Ordering::SeqCst) != published;
            let changed = whole && self.version.load(Ordering::SeqCst) != version;
            if !moved && !changed {
                return Some(value);
            }
        }
        None
    }

    /// The degree and weight at active position `index`, or `None` past the
    /// count. Any thread, never waiting: `None` too, but never in practice,
    /// when whole edits were published through every retry.
    // A scheduler's reads, wired up later in this stack.
    #[allow(dead_code)]
    pub(crate) fn position(&self, index: usize) -> Option<(i32, f32)> {
        self.consistent(READ_ATTEMPTS, false, |view| view.position(index))
            .flatten()
    }

    /// [`Self::position`], taking the edit lock should its retries run out.
    /// Control thread.
    pub(crate) fn position_or_wait(&self, index: usize) -> Option<(i32, f32)> {
        let read = self.consistent(READ_ATTEMPTS, false, |view| view.position(index));
        read.unwrap_or_else(|| {
            let _whole = self.whole();
            self.view().position(index)
        })
    }

    /// The active degrees and weights, in order. Control thread.
    fn copy(&self) -> (Vec<i32>, Vec<f32>) {
        debug_assert_control_thread("copying a melody's whole degree table");
        let mut degrees = [0; MAX_DEGREES];
        let mut weights = [0.0; MAX_DEGREES];
        loop {
            if let Some(count) = self.read_into(&mut degrees, &mut weights) {
                return (degrees[..count].to_vec(), weights[..count].to_vec());
            }
            std::thread::yield_now();
        }
    }

    /// The active degrees, in order. Control thread.
    pub(crate) fn degrees(&self) -> Vec<i32> {
        self.copy().0
    }

    /// The active weights, in order. Control thread.
    pub(crate) fn weights(&self) -> Vec<f32> {
        self.copy().1
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

    /// Marks a position write in flight, in the current epoch, until the
    /// guard drops. Taken before the write reads the generation it checks
    /// against, so an edit about to publish that generation again waits it
    /// out. Never waits: a retry means an edit moved the epoch meanwhile.
    fn in_flight(&self) -> InFlight<'_> {
        loop {
            let epoch = self.epoch.load(Ordering::SeqCst);
            let count = &self.writes[(epoch & 1) as usize];
            count.fetch_add(1, Ordering::SeqCst);
            if self.epoch.load(Ordering::SeqCst) == epoch {
                return InFlight(count);
            }
            // Marked after the epoch moved on: an edit may be waiting on
            // this count, and need not wait for this write.
            count.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Waits until every position write marked in flight so far has
    /// finished, should publishing `generation` call for it: moves the
    /// epoch on, so writes from now on mark the other count, and waits for
    /// the one before to empty. Control thread, under the edit lock; writes
    /// never wait for an edit, so this ends.
    fn drain_before(&self, generation: u32) {
        if !drains(generation) {
            return;
        }
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst);
        let count = &self.writes[(epoch & 1) as usize];
        while count.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
    }

    /// Publishes a generation with `store`, and counts the publish for
    /// readers to re-check.
    fn publish(&self, store: impl FnOnce()) {
        store();
        self.published.fetch_add(1, Ordering::SeqCst);
    }

    /// Writes `bits` to `slot` in `generation`, unless `now` has moved on
    /// from it: then the write came before the edit that moved it, and is
    /// forgotten with the rest. Never replaces a value of a later
    /// generation. Lock-free: a retry means another write landed. Call in
    /// flight (see [`Self::in_flight`]).
    fn write(&self, slot: &AtomicU64, generation: u32, now: impl Fn() -> u32, bits: u32) {
        let mut seen = slot.load(Ordering::SeqCst);
        while now() == generation {
            let value = tagged(generation, bits);
            match slot.compare_exchange_weak(seen, value, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(current) => seen = current,
            }
        }
        self.changed();
    }

    fn degrees_generation(&self) -> u32 {
        generation_of(self.header.load(Ordering::Acquire))
    }

    fn weights_generation(&self) -> u32 {
        self.weights_generation.load(Ordering::Acquire)
    }

    /// Writes the degree at active position `index` (clamped to ±127),
    /// returning false for a position past the count. Any thread.
    pub(crate) fn write_degree(&self, index: usize, value: i32) -> bool {
        let _in_flight = self.in_flight();
        let header = self.header.load(Ordering::SeqCst);
        self.write_degree_as_of(header, index, value)
    }

    /// [`Self::write_degree`], checked against and tagged with `header`.
    fn write_degree_as_of(&self, header: u64, index: usize, value: i32) -> bool {
        if index >= count_of(header) {
            return false;
        }
        let bits = value.clamp(-127, 127) as u32;
        let slot = &self.written_degrees[index];
        self.write(
            slot,
            generation_of(header),
            || self.degrees_generation(),
            bits,
        );
        true
    }

    /// Writes the weight at active position `index` (clamped to 0-10),
    /// returning false for a position past the count. Any thread.
    pub(crate) fn write_weight(&self, index: usize, value: f32) -> bool {
        // The weights' generation and the count as they stood together, so
        // a write checked against that count is tagged with that generation.
        let _in_flight = self.in_flight();
        loop {
            let generation = self.weights_generation.load(Ordering::SeqCst);
            let count = self.count();
            if self.weights_generation() == generation {
                return self.write_weight_as_of(generation, count, index, value);
            }
        }
    }

    /// [`Self::write_weight`], checked against `count` and tagged with
    /// `generation`.
    fn write_weight_as_of(&self, generation: u32, count: usize, index: usize, value: f32) -> bool {
        if index >= count {
            return false;
        }
        let bits = value.clamp(0.0, 10.0).to_bits();
        let slot = &self.written_weights[index];
        self.write(slot, generation, || self.weights_generation(), bits);
        true
    }

    /// Holds off whole edits. Control thread.
    fn whole(&self) -> MutexGuard<'_, ()> {
        debug_assert_control_thread("editing a melody's whole degree table");
        self.editing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes a whole edit (see the module docs). Control thread.
    fn edit(&self, edit: impl FnOnce(&Self)) {
        let _whole = self.whole();
        // Orders this edit's writes to a buffer after the generation that
        // retired it, for a reader still reading it to see that one.
        fence(Ordering::Release);
        edit(self);
        self.changed();
    }

    /// Sets the base scale; the count becomes its length, and degrees
    /// written to single positions are forgotten.
    pub(crate) fn set_scale(&self, mut degrees: Vec<i32>) {
        degrees.truncate(MAX_DEGREES);
        let bits: Vec<u32> = degrees.iter().map(|&degree| degree as u32).collect();
        self.edit(|table| {
            // Under the edit lock no other edit moves the generation; a
            // count set meanwhile is set before this scale.
            let next = next_generation(table.degrees_generation());
            table.drain_before(next);
            table.scales[buffer(next)].set(&bits);
            table.publish(|| {
                (table.header).store(header(next, bits.len()), Ordering::SeqCst);
            });
            forget(&table.written_degrees, next);
        });
    }

    /// Sets the base weights, forgetting weights written to single
    /// positions. Past [`MAX_DEGREES`] no position reads them.
    pub(crate) fn set_scale_weights(&self, mut weights: Vec<f32>) {
        weights.truncate(MAX_DEGREES);
        let bits: Vec<u32> = weights.iter().map(|weight| weight.to_bits()).collect();
        self.edit(|table| {
            let next = next_generation(table.weights_generation());
            table.drain_before(next);
            table.weights[buffer(next)].set(&bits);
            table.publish(|| (table.weights_generation).store(next, Ordering::SeqCst));
            forget(&table.written_weights, next);
        });
    }

    /// Restores degrees and weights written to single positions, active or
    /// not. Under the edit lock no edit moves the generations, so these
    /// writes need not be marked in flight.
    pub(crate) fn restore_written(
        &self,
        degrees: impl IntoIterator<Item = (usize, i32)>,
        weights: impl IntoIterator<Item = (usize, f32)>,
    ) {
        self.edit(|table| {
            let generation = table.degrees_generation();
            for (index, value) in degrees.into_iter().filter(|(i, _)| *i < MAX_DEGREES) {
                let bits = value.clamp(-127, 127) as u32;
                let slot = &table.written_degrees[index];
                table.write(slot, generation, || table.degrees_generation(), bits);
            }
            let generation = table.weights_generation();
            for (index, value) in weights.into_iter().filter(|(i, _)| *i < MAX_DEGREES) {
                let bits = value.clamp(0.0, 10.0).to_bits();
                let slot = &table.written_weights[index];
                table.write(slot, generation, || table.weights_generation(), bits);
            }
        });
    }

    /// Copies the active degrees and weights into `degrees` and `weights`,
    /// returning how many, or `None` when an edit was published or a
    /// position write finished during the copy: keep the copy before.
    /// Never waits.
    pub(crate) fn read_into(
        &self,
        degrees: &mut [i32; MAX_DEGREES],
        weights: &mut [f32; MAX_DEGREES],
    ) -> Option<usize> {
        self.consistent(1, true, |view| {
            let count = view.count();
            for i in 0..count {
                degrees[i] = view.degree(i);
                weights[i] = view.weight(i);
            }
            count
        })
    }

    /// Holds off whole edits until the guard is dropped, for tests of
    /// readers and writers that meet one.
    #[cfg(test)]
    pub(crate) fn editing(&self) -> MutexGuard<'_, ()> {
        self.whole()
    }
}

/// A position write in flight (see [`DegreeTable::in_flight`]): the count
/// it is marked in.
struct InFlight<'a>(&'a AtomicU64);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
