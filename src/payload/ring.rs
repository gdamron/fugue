//! A bounded single-producer, single-consumer ring (Lamport's queue) whose
//! producer never waits for its consumer.
//!
//! The producer owns `tail` and the consumer owns `head`; each only reads
//! the other's. A push compares the two, and either writes a free slot and
//! publishes it, or hands the value back at once. A slot the consumer has
//! not finished reading still counts as full, so a consumer preempted
//! mid-pop makes the ring look full rather than making the producer spin.
//!
//! One producer at a time is enforced by [`Producer::claim`], one consumer
//! at a time by a lock the producer never touches.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

pub(crate) struct Ring<T> {
    /// A power-of-two count of slots, indexed by counter modulo length.
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    /// Pops so far (wrapping). Written only by the consumer.
    head: AtomicUsize,
    /// Pushes so far (wrapping). Written only by the claimed producer.
    tail: AtomicUsize,
    producer: AtomicBool,
    consumer: Mutex<()>,
}

// SAFETY: the ring moves `T`s between threads, hence `T: Send`. Shared use
// is sound because every slot has one owner at a time: the producer (unique
// by `Producer::claim`, and `&mut` to push) from seeing it free until it
// publishes `tail`, then the consumer (unique under `consumer`) from seeing
// it filled until it publishes `head`.
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
    pub(crate) fn pop_paused(&self, paused: impl FnOnce()) -> Option<T> {
        let _consumer = self.consumer.lock().unwrap_or_else(PoisonError::into_inner);
        let head = self.head.load(Ordering::Relaxed);
        // Acquire pairs with the push's Release: the slot's value is visible.
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: `head != tail`, so slot `head` holds a published value; the
        // lock makes this the only consumer, and the producer leaves the slot
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
