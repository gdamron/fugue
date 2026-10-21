//! The degree table's parts: generations and the headers that pack them
//! with the count, values tagged with their generation, and base buffers.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::MAX_DEGREES;

/// The active count, from a header (low 32 bits).
pub(super) fn count_of(header: u64) -> usize {
    header as u32 as usize
}

/// The degrees' generation, from a header (high 32 bits).
pub(super) fn generation_of(header: u64) -> u32 {
    (header >> 32) as u32
}

pub(super) fn header(generation: u32, count: usize) -> u64 {
    u64::from(generation) << 32 | count as u64
}

/// The generation after `generation`: never 0, which marks a position never
/// written, and always of the other parity, which picks the other buffer.
pub(super) fn next_generation(generation: u32) -> u32 {
    generation.checked_add(1).unwrap_or(2)
}

/// Whether publishing `generation` waits out the writes in flight: twice a
/// round, half a round apart (see the module docs).
pub(super) fn drains(generation: u32) -> bool {
    generation == 2 || generation == 1 << 31
}

/// Which of two buffers `generation` reads its base from.
pub(super) fn buffer(generation: u32) -> usize {
    (generation & 1) as usize
}

pub(super) fn tagged(generation: u32, bits: u32) -> u64 {
    u64::from(generation) << 32 | u64::from(bits)
}

/// The value written to one position in `generation`, if any.
pub(super) fn written(slot: &AtomicU64, generation: u32) -> Option<u32> {
    let bits = slot.load(Ordering::SeqCst);
    (generation_of(bits) == generation).then_some(bits as u32)
}

/// Forgets the values in `slots` written before `generation`, so none is
/// read again should the generation come back round. Invisible to readers:
/// a value written before the current generation is already not read.
pub(super) fn forget(slots: &[AtomicU64], generation: u32) {
    for slot in slots {
        let _ = slot.fetch_update(Ordering::AcqRel, Ordering::Acquire, |bits| {
            (bits != 0 && generation_of(bits) != generation).then_some(0)
        });
    }
}

/// A base scale or base weights: entries (`i32` or `f32` bits) and how many.
pub(super) struct Base {
    pub(super) entries: [AtomicU32; MAX_DEGREES],
    pub(super) len: AtomicU32,
}

impl Base {
    pub(super) fn new(values: &[u32]) -> Self {
        let base = Self {
            entries: std::array::from_fn(|_| AtomicU32::new(0)),
            len: AtomicU32::new(0),
        };
        base.set(values);
        base
    }

    /// Published by the generation that reads it, stored after.
    pub(super) fn set(&self, values: &[u32]) {
        for (slot, &value) in self.entries.iter().zip(values) {
            slot.store(value, Ordering::Relaxed);
        }
        self.len.store(values.len() as u32, Ordering::Relaxed);
    }

    pub(super) fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed) as usize
    }

    pub(super) fn get(&self, i: usize) -> u32 {
        self.entries[i].load(Ordering::Relaxed)
    }
}
