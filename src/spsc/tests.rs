//! The ring: FIFO across threads, one producer at a time, leftovers freed,
//! a producer that never waits for a stalled consumer, and a pop that
//! releases its slot exactly once even when it unwinds.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use super::{Producer, Ring};
use crate::alloc_counter::allocator_events;

/// Records which thread drops each [`Tracked`] value, in order.
#[derive(Clone, Default)]
struct Ledger(Arc<Mutex<Vec<(usize, ThreadId)>>>);

impl Ledger {
    fn value(&self, id: usize) -> Tracked {
        Tracked {
            id,
            ledger: self.clone(),
        }
    }

    fn drops(&self) -> Vec<(usize, ThreadId)> {
        self.0.lock().unwrap().clone()
    }

    fn dropped_ids(&self) -> Vec<usize> {
        self.drops().into_iter().map(|(id, _)| id).collect()
    }
}

struct Tracked {
    id: usize,
    ledger: Ledger,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        let dropper = thread::current().id();
        self.ledger.0.lock().unwrap().push((self.id, dropper));
    }
}

#[test]
fn moves_every_value_once_in_order_across_threads() {
    const COUNT: usize = if cfg!(miri) { 200 } else { 20_000 };
    let ledger = Ledger::default();
    let ring = Ring::with_capacity(4);
    let mut producer = Producer::claim(Arc::clone(&ring));
    let consumer = thread::scope(|scope| {
        scope.spawn(|| {
            for id in 0..COUNT {
                let mut value = ledger.value(id);
                while let Err(back) = producer.push(value) {
                    value = back;
                    thread::yield_now();
                }
            }
        });
        scope
            .spawn(|| {
                let mut next = 0;
                while next < COUNT {
                    match ring.pop() {
                        Some(value) => {
                            assert_eq!(value.id, next);
                            next += 1;
                        }
                        None => thread::yield_now(),
                    }
                }
                thread::current().id()
            })
            .join()
            .unwrap()
    });
    assert!(ring.pop().is_none());
    let drops = ledger.drops();
    assert_eq!(drops.len(), COUNT);
    assert!(drops
        .iter()
        .enumerate()
        .all(|(index, &(id, dropper))| id == index && dropper == consumer));
}

#[test]
#[should_panic(expected = "already has a producer")]
fn a_second_producer_panics() {
    let ring = Ring::<u32>::with_capacity(1);
    let _first = Producer::claim(Arc::clone(&ring));
    let _second = Producer::claim(ring);
}

#[test]
fn a_new_producer_continues_from_the_last_ones_tail() {
    let ring = Ring::with_capacity(4);
    let mut first = Producer::claim(Arc::clone(&ring));
    first.push(1).unwrap();
    drop(first);
    let second = Arc::clone(&ring);
    thread::spawn(move || Producer::claim(second).push(2).unwrap())
        .join()
        .unwrap();
    assert_eq!(ring.pop(), Some(1));
    assert_eq!(ring.pop(), Some(2));
    assert_eq!(ring.pop(), None);
}

#[test]
fn dropping_the_ring_frees_leftovers() {
    let ledger = Ledger::default();
    let ring = Ring::with_capacity(4);
    let mut producer = Producer::claim(Arc::clone(&ring));
    for id in 1..=3 {
        assert!(producer.push(ledger.value(id)).is_ok());
    }
    assert_eq!(ring.pop().map(|value| value.id), Some(1));
    drop((producer, ring));
    assert_eq!(ledger.dropped_ids(), [1, 2, 3]);
}

#[test]
fn a_stalled_consumer_never_makes_the_producer_wait() {
    let ring = Ring::with_capacity(1);
    let mut producer = Producer::claim(Arc::clone(&ring));
    producer.push(1usize).unwrap();

    // The consumer has moved value 1 out but not released its slot: the
    // producer, on another thread, finds the ring full at once.
    let popped = ring.pop_paused(|| {
        thread::scope(|scope| {
            scope.spawn(|| {
                let (refused, allocs, frees) = allocator_events(|| producer.push(2));
                assert_eq!(refused, Err(2));
                assert_eq!((allocs, frees), (0, 0));
            });
        });
    });
    assert_eq!(popped, Some(1));
    assert_eq!(producer.push(2), Ok(()));
    assert_eq!(ring.pop(), Some(2));
}

#[test]
fn a_pop_that_unwinds_releases_its_slot_once() {
    let ledger = Ledger::default();
    let ring = Ring::with_capacity(1);
    let mut producer = Producer::claim(Arc::clone(&ring));
    assert!(producer.push(ledger.value(1)).is_ok());

    let unwound = catch_unwind(AssertUnwindSafe(|| {
        ring.pop_paused(|| panic!("consumer failed mid-pop"))
    }));
    assert!(unwound.is_err());
    assert_eq!(ledger.dropped_ids(), [1]);

    // The slot was released: the ring is empty and takes a new value.
    assert!(ring.pop().is_none());
    assert!(producer.push(ledger.value(2)).is_ok());
    drop((producer, ring));
    assert_eq!(ledger.dropped_ids(), [1, 2]);
}
