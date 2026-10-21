//! The degree table as a scheduler reads it on the audio thread: copied
//! into atomics after every edit, while the edit still holds the table, and
//! read without the lock. A ramp into a degree, a weight or the count starts
//! from it, so a control thread holding the table never loses a ramp.

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

use super::{DegreeTable, MAX_DEGREES};

pub(crate) struct TableMirror {
    count: AtomicUsize,
    degrees: Box<[AtomicI32]>,
    weights: Box<[AtomicU32]>,
}

impl TableMirror {
    pub(super) fn new(table: &DegreeTable) -> Self {
        let mirror = Self {
            count: AtomicUsize::new(0),
            degrees: (0..MAX_DEGREES).map(|_| AtomicI32::new(0)).collect(),
            weights: (0..MAX_DEGREES).map(|_| AtomicU32::new(0)).collect(),
        };
        mirror.publish(table);
        mirror
    }

    /// Copies `table` in. Its caller holds the table, so publishes never
    /// interleave; a reader between two may see some of each.
    pub(super) fn publish(&self, table: &DegreeTable) {
        for (slot, &degree) in self.degrees.iter().zip(&table.degrees) {
            slot.store(degree, Ordering::Relaxed);
        }
        for (slot, &weight) in self.weights.iter().zip(&table.weights) {
            slot.store(weight.to_bits(), Ordering::Relaxed);
        }
        self.count.store(table.degrees.len(), Ordering::Release);
    }

    pub(super) fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    /// The degree at active position `index`, if there is one.
    pub(super) fn degree(&self, index: usize) -> Option<i32> {
        (index < self.count()).then(|| self.degrees[index].load(Ordering::Relaxed))
    }

    /// The weight at active position `index`, if there is one.
    pub(super) fn weight(&self, index: usize) -> Option<f32> {
        (index < self.count()).then(|| f32::from_bits(self.weights[index].load(Ordering::Relaxed)))
    }
}
