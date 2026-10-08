//! Exhaustive interleaving models of the request queue's ordering
//! arguments.
//!
//! The queue and event-counter source files are compiled a second time here
//! against loom's atomics and `UnsafeCell` (the `sync` module below replaces
//! the crate's `std` one), so the models check the exact code that ships and
//! run in a plain `cargo test --lib control_request::loom_tests`. Each test
//! explores the interleavings and weak-memory outcomes loom models, and
//! loom fails a model if two slot accesses race (are not ordered by
//! happens-before).
//!
//! The shim's [`AtomicU64`](sync::AtomicU64) closes a gap in loom's
//! partial-order reduction: loom remembers only the *last* access to an
//! atomic, so when thread A loads a slot's `seq` and thread B then loads
//! and stores it, B's store is checked against its own load only, and the
//! schedule where B's store precedes A's load is never explored. That is
//! exactly the queue's pattern (each side loads `seq`, then stores it), and
//! without the shim no model explores a `pop` that sees a concurrent push,
//! so weakening any ordering passes. See [`sync::AtomicU64`].
// The source files are deliberately loaded a second time, and not every item
// they define is exercised here.
#![allow(clippy::duplicate_mod, dead_code)]

mod sync {
    pub(super) use loom::cell::UnsafeCell;
    pub(super) use loom::sync::atomic::{AtomicU32, Ordering};
    pub(super) use loom::sync::Arc;
    pub(super) use loom::thread::yield_now as spin_loop;

    /// loom's `AtomicU64`, with every access also made a `Relaxed` RMW of
    /// a shadow atomic, so loom treats any two accesses as dependent and
    /// explores both of their orders (see the module docs). `Relaxed` RMWs
    /// create no happens-before edge, and the value cell's semantics are
    /// untouched, so this adds schedules without hiding any race.
    pub(super) struct AtomicU64 {
        value: loom::sync::atomic::AtomicU64,
        shadow: loom::sync::atomic::AtomicU64,
    }

    impl AtomicU64 {
        pub(super) fn new(value: u64) -> Self {
            Self {
                value: loom::sync::atomic::AtomicU64::new(value),
                shadow: loom::sync::atomic::AtomicU64::new(0),
            }
        }

        fn conflict(&self) {
            self.shadow.fetch_add(1, Ordering::Relaxed);
        }

        pub(super) fn load(&self, order: Ordering) -> u64 {
            self.conflict();
            self.value.load(order)
        }

        pub(super) fn store(&self, value: u64, order: Ordering) {
            self.conflict();
            self.value.store(value, order);
        }

        pub(super) fn compare_exchange(
            &self,
            current: u64,
            new: u64,
            success: Ordering,
            failure: Ordering,
        ) -> Result<u64, u64> {
            self.conflict();
            self.value.compare_exchange(current, new, success, failure)
        }
    }
}

#[path = "event.rs"]
mod event;
#[path = "queue.rs"]
mod queue;

use loom::sync::atomic::{AtomicU32, Ordering};
use loom::sync::Arc;
use loom::thread;

use event::{EventCounter, EventCursor};
use queue::{bounded, QueueConsumer, QueueProducer};

/// Three-thread models bound preemptions to keep each to seconds; the
/// two-thread models are unbounded.
const PREEMPTIONS: usize = 3;

/// Spawns a producer that pushes `values` in order, returning the ones the
/// queue refused.
fn spawn_producer(
    producer: &QueueProducer<u32>,
    values: &'static [u32],
) -> thread::JoinHandle<Vec<u32>> {
    let producer = producer.clone();
    thread::spawn(move || {
        values
            .iter()
            .filter_map(|&value| producer.try_push(value).err())
            .collect()
    })
}

fn drain(consumer: &mut QueueConsumer<u32>, received: &mut Vec<u32>) {
    while let Some(value) = consumer.pop() {
        received.push(value);
    }
}

/// Asserts that `received` and `refused` together hold every value of
/// `pushed` exactly once, and that each producer's received values kept
/// their push order.
fn assert_exactly_once(pushed: &[&[u32]], received: &[u32], refused: &[u32]) {
    let mut all: Vec<u32> = received.iter().chain(refused).copied().collect();
    all.sort_unstable();
    let mut expected: Vec<u32> = pushed
        .iter()
        .flat_map(|values| values.iter())
        .copied()
        .collect();
    expected.sort_unstable();
    assert_eq!(all, expected, "received {received:?}, refused {refused:?}");
    for values in pushed {
        let order: Vec<u32> = received
            .iter()
            .filter(|v| values.contains(v))
            .copied()
            .collect();
        assert!(order.is_sorted(), "producer order broken: {received:?}");
    }
}

/// Two producers push while the consumer pops: every value arrives exactly
/// once, in each producer's order, and every slot handoff is race-free.
#[test]
fn concurrent_producers_deliver_every_value_once_in_order() {
    const A: &[u32] = &[1, 2];
    const B: &[u32] = &[11];
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(PREEMPTIONS);
    model.check(|| {
        let (producer, mut consumer) = bounded(4);
        let a = spawn_producer(&producer, A);
        let b = spawn_producer(&producer, B);
        let mut received = Vec::new();
        drain(&mut consumer, &mut received);
        let mut refused = a.join().unwrap();
        refused.extend(b.join().unwrap());
        drain(&mut consumer, &mut received);
        assert!(refused.is_empty(), "a queue of 4 refused {refused:?}");
        assert_exactly_once(&[A, B], &received, &refused);
    });
}

/// Three pushes into a queue of two while the consumer pops once: the
/// refused values are exactly the ones not received.
#[test]
fn overflow_hands_back_exactly_the_values_not_received() {
    const A: &[u32] = &[1, 2];
    const B: &[u32] = &[11];
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(PREEMPTIONS);
    model.check(|| {
        let (producer, mut consumer) = bounded(2);
        let a = spawn_producer(&producer, A);
        let b = spawn_producer(&producer, B);
        let mut received = Vec::from_iter(consumer.pop());
        let mut refused = a.join().unwrap();
        refused.extend(b.join().unwrap());
        drain(&mut consumer, &mut received);
        assert_exactly_once(&[A, B], &received, &refused);
    });
}

/// One producer laps a queue of two while the consumer pops: a slot is
/// rewritten only after its previous value was read (loom checks the slot
/// accesses), and values arrive in order.
#[test]
fn slots_are_reused_only_after_the_read() {
    const A: &[u32] = &[1, 2, 3];
    loom::model(|| {
        let (producer, mut consumer) = bounded(2);
        let a = spawn_producer(&producer, A);
        let mut received = Vec::new();
        for _ in 0..2 {
            received.extend(consumer.pop());
        }
        let refused = a.join().unwrap();
        drain(&mut consumer, &mut received);
        assert_exactly_once(&[A], &received, &refused);
    });
}

/// A peek racing a producer that laps a queue of two sees nothing or the
/// head value, which the next pop returns: peeking never reads a slot the
/// producer is rewriting (loom checks the slot accesses).
#[test]
fn a_peek_sees_nothing_or_the_value_the_next_pop_returns() {
    const A: &[u32] = &[1, 2, 3];
    loom::model(|| {
        let (producer, mut consumer) = bounded(2);
        let a = spawn_producer(&producer, A);
        let mut received = Vec::new();
        for _ in 0..2 {
            if let Some(head) = consumer.peek(|value| *value) {
                assert_eq!(consumer.pop(), Some(head));
                received.push(head);
            }
        }
        let refused = a.join().unwrap();
        drain(&mut consumer, &mut received);
        assert_exactly_once(&[A], &received, &refused);
    });
}

/// Items left in the queue are dropped once, by whichever handle goes last.
#[test]
fn the_last_handle_drops_unpopped_items() {
    loom::model(|| {
        let drops = Arc::new(AtomicU32::new(0));
        struct Tracked(Arc<AtomicU32>);
        impl Drop for Tracked {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let (producer, mut consumer) = bounded(2);
        let pusher = {
            let drops = drops.clone();
            thread::spawn(move || {
                for _ in 0..2 {
                    let _ = producer.try_push(Tracked(drops.clone()));
                }
            })
        };
        drop(consumer.pop());
        drop(consumer);
        pusher.join().unwrap();
        assert_eq!(drops.load(Ordering::Relaxed), 2);
    });
}

/// Concurrent events are all counted, and a cursor that sees an event sees
/// what its recorder stored before it.
#[test]
fn every_event_is_counted_and_publishes_earlier_stores() {
    loom::model(|| {
        let counter = Arc::new(EventCounter::new());
        let payloads = Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
        let recorders: Vec<_> = (0..2)
            .map(|i| {
                let (counter, payloads) = (counter.clone(), payloads.clone());
                thread::spawn(move || {
                    payloads[i].store(1, Ordering::Relaxed);
                    counter.record();
                })
            })
            .collect();

        let mut cursor = EventCursor::new();
        let seen = cursor.take(&counter);
        let published = payloads
            .iter()
            .filter(|payload| payload.load(Ordering::Relaxed) == 1)
            .count() as u32;
        assert!(
            published >= seen,
            "{seen} events seen, {published} payloads"
        );

        for recorder in recorders {
            recorder.join().unwrap();
        }
        assert_eq!(seen + cursor.take(&counter), 2, "an event was lost");
    });
}
