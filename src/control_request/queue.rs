//! A bounded, allocation-free multi-producer, single-consumer queue.
//!
//! # Why one MPSC queue
//!
//! Producers are dynamic: RPC handlers, script threads, agents and the audio
//! thread's own automation come and go. Per-producer SPSC rings would need
//! each producer registered (and its ring allocated) before it could write,
//! and merging rings on the audio side has no single arrival order to merge
//! by. One MPSC queue has one: the order in which pushes claim positions,
//! which is the order "last write wins" coalescing needs.
//!
//! std's bounded channel is not a fit either: its `try_send` and
//! `try_recv` can spin or yield while a peer is preempted mid-operation,
//! and it is not our source, so loom cannot check it. This queue compiles
//! against the `sync` shim, and `loom_tests` model-checks this exact file.
//!
//! # Protocol
//!
//! This is Dmitry Vyukov's bounded array queue with the consumer side
//! specialised to one thread. Positions are `u64` stamps counting up from 0
//! (wrapping at `u64::MAX`); position `pos` lives in slot `pos % capacity`.
//! Each slot's
//! `seq` says what the slot is waiting for:
//!
//! - `seq == pos`: free for the producer of `pos`;
//! - `seq == pos + 1`: holds the value pushed at `pos`, for the consumer;
//! - after the read, the consumer stores `pos + capacity`: free for the
//!   producer of the next lap.
//!
//! A producer claims `pos` by moving the shared `tail` from `pos` to
//! `pos + 1` with a compare-exchange, writes the value, then publishes it.
//! The consumer's `head` is its own (a plain `u64`): it reads the slot at
//! `head` only once that slot's `seq` is `head + 1`.
//!
//! # Ordering
//!
//! Each slot's value is handed between threads by its `seq` alone: the
//! producer's `Release` store of `pos + 1` makes its write happen-before the
//! consumer's read (the consumer's `Acquire` load saw it), and the
//! consumer's `Release` store of `pos + capacity` makes its read
//! happen-before the next lap's write (that producer's `Acquire` load saw
//! it, before its compare-exchange). The `tail` compare-exchange only hands
//! out positions: RMW atomicity alone gives each `pos` to exactly one
//! producer, so it is `Relaxed`, and it orders no data.
//!
//! # Why 64-bit stamps on every target
//!
//! The compare-exchange is open to ABA: a producer paused after loading
//! `seq == pos` and before its exchange would win a stale exchange if the
//! other threads completed a full cycle of positions meanwhile, landing
//! `tail` on `pos` again; it would then overwrite an unconsumed value and
//! wedge the consumer. No memory ordering prevents that; only making the
//! cycle unreachable does. With `usize` positions a 32-bit target (wasm32)
//! cycles after 2^32 pushes, so positions and sequences are `u64` on every
//! target: a cycle takes 2^64 pushes, centuries at 10^9 pushes a second.
//!
//! # Progress
//!
//! [`QueueProducer::try_push`] is lock-free (it retries only when another
//! producer claimed the position first) and [`QueueConsumer::pop`] is
//! wait-free. A producer preempted between claiming a position and
//! publishing it makes `pop` report empty at that position, so items behind
//! it wait until it publishes: order is kept, and the consumer never waits.

use std::mem::MaybeUninit;

use super::sync::{debug_assert_control_thread, spin_loop, Arc, AtomicU64, Ordering, UnsafeCell};

/// Creates a queue holding up to `capacity` items, rounded up to a power of
/// two and at least 2 (one lap must tell a full slot from a free one).
/// Allocates once: call it on a control thread.
///
/// # Panics
///
/// If the rounded capacity overflows `usize`.
pub(crate) fn bounded<T: Send>(capacity: usize) -> (QueueProducer<T>, QueueConsumer<T>) {
    bounded_from(capacity, 0)
}

/// [`bounded`], with positions counting from `start`, so tests can reach
/// the `u64` wrap.
pub(super) fn bounded_from<T: Send>(
    capacity: usize,
    start: u64,
) -> (QueueProducer<T>, QueueConsumer<T>) {
    let capacity = capacity
        .max(2)
        .checked_next_power_of_two()
        .expect("queue capacity overflows usize");
    let mask = capacity as u64 - 1;
    let slots = (0..capacity as u64)
        .map(|index| Slot {
            // The first position from `start` that lands in this slot.
            seq: AtomicU64::new(start.wrapping_add(index.wrapping_sub(start) & mask)),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        })
        .collect();
    let shared = Arc::new(Shared {
        slots,
        mask,
        tail: AtomicU64::new(start),
    });
    let producer = QueueProducer {
        shared: shared.clone(),
    };
    (
        producer,
        QueueConsumer {
            shared,
            head: start,
        },
    )
}

struct Slot<T> {
    seq: AtomicU64,
    value: UnsafeCell<MaybeUninit<T>>,
}

struct Shared<T> {
    slots: Box<[Slot<T>]>,
    /// `capacity - 1`; the capacity is a power of two.
    mask: u64,
    /// The next position a producer claims.
    tail: AtomicU64,
}

// SAFETY: the queue moves `T` values between threads and never shares a
// `&T`, so it is as thread-safe as sending `T` itself.
unsafe impl<T: Send> Send for Shared<T> {}
// SAFETY: a slot's value is accessed by one thread at a time, as the
// protocol hands it over: the producer that won its position, between the
// compare-exchange and its `Release` store of `seq`; then the consumer,
// between its `Acquire` load of that `seq` and its own `Release` store.
unsafe impl<T: Send> Sync for Shared<T> {}

impl<T> Shared<T> {
    #[inline]
    fn slot(&self, pos: u64) -> &Slot<T> {
        &self.slots[(pos & self.mask) as usize]
    }
}

impl<T> Drop for Shared<T> {
    /// Drops the items pushed and not popped. Runs when the last handle is
    /// dropped, which frees the slots: a control-thread operation.
    fn drop(&mut self) {
        debug_assert_control_thread("freeing a request queue");
        for (index, slot) in (0u64..).zip(self.slots.iter()) {
            // `&mut self`: every other handle is gone, and the `Arc` drop
            // that got here synchronized with each of them, so `Relaxed`
            // sees every push and pop. A slot is full exactly when its
            // `seq` is one past a position it holds.
            if slot.seq.load(Ordering::Relaxed).wrapping_sub(index) & self.mask == 1 {
                // SAFETY: a full slot holds an initialized value nobody
                // read; it is dropped once, here.
                slot.value
                    .with_mut(|cell| unsafe { (*cell).assume_init_drop() });
            }
        }
    }
}

/// The pushing end of a [`bounded`] queue. Clone it for each producer.
pub(crate) struct QueueProducer<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for QueueProducer<T> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl<T> QueueProducer<T> {
    /// Whether at least `reserve` slots past the next position are free,
    /// so a push now leaves them for others. Exact while pushes are
    /// serialized (a push racing this check can take one of them); the
    /// consumer only frees slots. `reserve` must be below the capacity.
    /// Lock-free and allocation-free.
    pub(crate) fn leaves(&self, reserve: u64) -> bool {
        let shared = &*self.shared;
        debug_assert!(reserve <= shared.mask, "a reserve must leave a slot to push");
        // The consumer frees slots in position order, so the slot `reserve`
        // ahead being free for its position (seq == position, as for a
        // push) means every slot up to it is.
        let ahead = shared.tail.load(Ordering::Relaxed).wrapping_add(reserve);
        shared.slot(ahead).seq.load(Ordering::Acquire) == ahead
    }

    /// Pushes `value`, or hands it back if the queue is full. Lock-free and
    /// allocation-free from any thread, including the audio thread.
    ///
    /// "Full" means the slot for the next position still holds the item
    /// pushed one lap earlier. Under contention a concurrent `pop` may have
    /// just made room, as with any bounded queue.
    pub(crate) fn try_push(&self, value: T) -> Result<(), T> {
        let shared = &*self.shared;
        let mut pos = shared.tail.load(Ordering::Relaxed);
        loop {
            let slot = shared.slot(pos);
            let seq = slot.seq.load(Ordering::Acquire);
            // Laps the slot is ahead of (+) or behind (-) `pos`, wrapping so
            // that positions overflowing `u64` are harmless.
            let lag = seq.wrapping_sub(pos) as i64;
            if lag == 0 {
                match shared.tail.compare_exchange(
                    pos,
                    pos.wrapping_add(1),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // SAFETY: winning the exchange made this producer
                        // the only writer of `pos`, and the `Acquire` load
                        // above saw the previous lap's read finish.
                        slot.value
                            .with_mut(|cell| unsafe { cell.write(MaybeUninit::new(value)) });
                        slot.seq.store(pos.wrapping_add(1), Ordering::Release);
                        return Ok(());
                    }
                    Err(current) => pos = current,
                }
            } else if lag < 0 {
                return Err(value);
            } else {
                // Another producer claimed `pos` after our `tail` load.
                spin_loop();
                pos = shared.tail.load(Ordering::Relaxed);
            }
        }
    }
}

/// The popping end of a [`bounded`] queue: one consumer, the audio thread.
pub(crate) struct QueueConsumer<T> {
    shared: Arc<Shared<T>>,
    /// The next position to read.
    head: u64,
}

impl<T> QueueConsumer<T> {
    /// Calls `f` with the oldest published item, leaving it queued, or
    /// returns `None` exactly when [`Self::pop`] would. Wait-free,
    /// allocation-free and never frees.
    ///
    /// Takes `&mut self` although it only reads: the consumer is `Sync`
    /// whenever `T: Send`, so a `&self` peek would let two threads hold
    /// `&T` at once for a `T` that is not `Sync` (a `Cell`, say).
    pub(crate) fn peek<R>(&mut self, f: impl FnOnce(&T) -> R) -> Option<R> {
        let slot = self.shared.slot(self.head);
        if slot.seq.load(Ordering::Acquire) != self.head.wrapping_add(1) {
            return None;
        }
        // SAFETY: as in `pop`, the value is initialized and its write
        // happens-before this read. The slot stays the consumer's until its
        // `Release` store in `pop`, which cannot run while `self` is
        // borrowed, so no producer writes it during `f`; and `&mut self`
        // makes this the only reference to it.
        Some(
            slot.value
                .with(|cell| f(unsafe { (*cell).assume_init_ref() })),
        )
    }

    /// Pops the oldest published item, or `None` if the next one is not
    /// published yet. Wait-free, allocation-free and never frees.
    pub(crate) fn pop(&mut self) -> Option<T> {
        let shared = &*self.shared;
        let slot = shared.slot(self.head);
        if slot.seq.load(Ordering::Acquire) != self.head.wrapping_add(1) {
            return None;
        }
        // SAFETY: `seq == head + 1` means the producer of `head` published
        // an initialized value, and its `Release` store happens-before this
        // read. Storing the next lap's `seq` below gives up the slot, so the
        // value is moved out once.
        let value = slot.value.with(|cell| unsafe { cell.read().assume_init() });
        let next_lap = self.head.wrapping_add(shared.mask + 1);
        slot.seq.store(next_lap, Ordering::Release);
        self.head = self.head.wrapping_add(1);
        Some(value)
    }
}
