//! The crate's single-producer, single-consumer ring with a wait-free
//! producer, for handing values across the audio-thread boundary (Lamport's
//! queue).
//!
//! The producer owns `tail` and the consumer owns `head`; each only reads
//! the other's. A push compares the two, and either writes a free slot and
//! publishes it, or hands the value back at once. A slot the consumer has
//! not finished reading still counts as full, so a consumer preempted
//! mid-pop makes the ring look full rather than making the producer spin.
//! Pushing allocates, frees and locks nothing, and never waits.
//!
//! One producer at a time is enforced by [`Producer::claim`]. The consumer
//! side is either one claimed [`Consumer`], whose pop takes no lock and
//! never waits (for the audio thread), or [`Ring::pop`]'s callers,
//! serialized by a lock the producer never touches (for control threads).
//! A popping consumer is the mirror image of a pushing producer: a producer
//! preempted mid-push makes the ring look empty at that slot rather than
//! making the consumer spin.

// Not every user calls every method outside tests.
#![cfg_attr(not(test), allow(dead_code))]

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// A bounded ring of `T`s: one [`Producer`] pushes, consumers pop.
pub(crate) struct Ring<T> {
    /// A power-of-two count of slots, indexed by counter modulo length.
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    /// Pops so far (wrapping). Written only by the consumer.
    head: AtomicUsize,
    /// Pushes so far (wrapping). Written only by the claimed producer.
    tail: AtomicUsize,
    producer: AtomicBool,
    /// Serializes [`Ring::pop`] callers, and [`Consumer::claim`] with them.
    consumer: Mutex<()>,
    /// Whether a [`Consumer`] holds the consumer end; set and checked only
    /// under `consumer`, cleared by the consumer's drop.
    claimed: AtomicBool,
}

// SAFETY: the ring moves `T`s between threads, hence `T: Send`. Shared use
// is sound because every slot has one owner at a time: the producer (unique
// by `Producer::claim`, and `&mut` to push) from seeing it free until it
// publishes `tail`, then the consumer (the claimed `Consumer`, `&mut` to
// pop, or else the one holder of `consumer`) from seeing it filled until it
// publishes `head`.
unsafe impl<T: Send> Sync for Ring<T> {}

impl<T> Ring<T> {
    /// A ring of at least `capacity` slots. Control thread only: allocates.
    pub(crate) fn with_capacity(capacity: usize) -> Arc<Self> {
        let len = capacity.max(1).next_power_of_two();
        Arc::new(Self {
            slots: (0..len)
                .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
                .collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            producer: AtomicBool::new(false),
            consumer: Mutex::new(()),
            claimed: AtomicBool::new(false),
        })
    }

    fn slot(&self, counter: usize) -> &UnsafeCell<MaybeUninit<T>> {
        &self.slots[counter & (self.slots.len() - 1)]
    }

    /// Consumer: takes the oldest value. Control side: takes a lock.
    pub(crate) fn pop(&self) -> Option<T> {
        self.pop_paused(|| {})
    }

    /// As [`Self::pop`], running `paused` after the value is read but
    /// before its slot is released: a consumer preempted mid-pop.
    ///
    /// # Panics
    ///
    /// If a [`Consumer`] holds the consumer end: a programming error.
    pub(crate) fn pop_paused(&self, paused: impl FnOnce()) -> Option<T> {
        let _consumer = self.consumer.lock().unwrap_or_else(PoisonError::into_inner);
        // Acquire pairs with the last claimed consumer's release of its
        // claim, so this pop starts from its `head`.
        assert!(
            !self.claimed.load(Ordering::Acquire),
            "this queue's consumer is claimed"
        );
        // SAFETY: the lock makes this the only `pop`, and no `Consumer`
        // exists: one claims only under the lock.
        unsafe { self.take(paused) }
    }

    /// Takes the oldest value, running `paused` between reading it and
    /// releasing its slot. Allocates, frees and locks nothing, and never
    /// waits.
    ///
    /// # Safety
    ///
    /// The caller must be the only consumer while this runs.
    unsafe fn take(&self, paused: impl FnOnce()) -> Option<T> {
        let head = self.head.load(Ordering::Relaxed);
        // Acquire pairs with the push's Release: the slot's value is visible.
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: `head != tail`, so slot `head` holds a published value; the
        // caller is the only consumer, and the producer leaves the slot
        // alone until `head` passes it.
        let value = unsafe { (*self.slot(head).get()).assume_init_read() };
        // Releases the slot even if `paused` unwinds, so it is never read twice.
        let _release = ReleaseSlot(&self.head, head.wrapping_add(1));
        paused();
        Some(value)
    }

    /// Control thread: drops every value pushed so far, each after
    /// releasing the consumer lock, and returns how many.
    pub(crate) fn drain(&self) -> usize {
        let mut freed = 0;
        while let Some(value) = self.pop() {
            drop(value);
            freed += 1;
        }
        freed
    }
}

/// Publishes a pop's new `head` when dropped.
struct ReleaseSlot<'a>(&'a AtomicUsize, usize);

impl Drop for ReleaseSlot<'_> {
    fn drop(&mut self) {
        // Release pairs with the push's Acquire: the read is done.
        self.0.store(self.1, Ordering::Release);
    }
}

impl<T> Drop for Ring<T> {
    fn drop(&mut self) {
        self.drain();
    }
}

/// The ring's one producer. Pushing allocates, frees and locks nothing,
/// and never waits.
pub(crate) struct Producer<T> {
    ring: Arc<Ring<T>>,
}

impl<T> Producer<T> {
    /// Claims `ring`'s producer end until this is dropped.
    ///
    /// # Panics
    ///
    /// If the ring already has a producer: a control-thread programming
    /// error.
    pub(crate) fn claim(ring: Arc<Ring<T>>) -> Self {
        // Acquire pairs with the previous producer's release of its claim,
        // so this one starts from its `tail`.
        let claimed = ring.producer.swap(true, Ordering::AcqRel);
        assert!(!claimed, "this queue already has a producer");
        Self { ring }
    }

    /// Queues `value`, or hands it back at once when the ring is full.
    pub(crate) fn push(&mut self, value: T) -> Result<(), T> {
        self.push_paused(value, || {})
    }

    /// As [`Self::push`], running `paused` after the value is written but
    /// before it is published: a producer preempted mid-push.
    pub(crate) fn push_paused(&mut self, value: T, paused: impl FnOnce()) -> Result<(), T> {
        let ring = &*self.ring;
        let tail = ring.tail.load(Ordering::Relaxed);
        // Acquire pairs with the pop's Release: freed slots are done with.
        if tail.wrapping_sub(ring.head.load(Ordering::Acquire)) == ring.slots.len() {
            return Err(value);
        }
        // SAFETY: fewer than `len` values are outstanding, so slot `tail` is
        // free (released by the consumer, or never used), and only this
        // claimed producer writes slots.
        unsafe { (*ring.slot(tail).get()).write(value) };
        paused();
        // Release pairs with the pop's Acquire: the value is published.
        ring.tail.store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl<T> Drop for Producer<T> {
    fn drop(&mut self) {
        self.ring.producer.store(false, Ordering::Release);
    }
}

/// The ring's one lock-free consumer, for the audio thread. Popping
/// allocates, locks and waits for nothing, and frees nothing but what the
/// caller drops.
pub(crate) struct Consumer<T> {
    ring: Arc<Ring<T>>,
}

impl<T> Consumer<T> {
    /// Claims `ring`'s consumer end until this is dropped; [`Ring::pop`]
    /// panics meanwhile. Control thread only: takes the consumer lock.
    ///
    /// # Panics
    ///
    /// If the ring already has a `Consumer`: a control-thread programming
    /// error.
    pub(crate) fn claim(ring: Arc<Ring<T>>) -> Self {
        let lock = ring.consumer.lock().unwrap_or_else(PoisonError::into_inner);
        // The lock orders this after every earlier `Ring::pop`, and Acquire
        // pairs with an earlier consumer's release of its claim: either way
        // this one starts from the last `head`.
        let claimed = ring.claimed.swap(true, Ordering::AcqRel);
        drop(lock);
        assert!(!claimed, "this queue already has a consumer");
        Self { ring }
    }

    /// Takes the oldest value, or `None` at once when there is none
    /// published, including while the producer is preempted mid-push.
    pub(crate) fn pop(&mut self) -> Option<T> {
        // SAFETY: the claim makes this the only consumer, and `&mut self`
        // keeps it to one call at a time.
        unsafe { self.ring.take(|| {}) }
    }
}

impl<T> Drop for Consumer<T> {
    fn drop(&mut self) {
        // Release pairs with the next consumer's Acquire: its `head` is ours.
        self.ring.claimed.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
