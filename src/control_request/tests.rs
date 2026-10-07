//! Unit and allocation tests for the request queue. The interleaving
//! arguments are modelled exhaustively in `loom_tests`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use super::queue::bounded_from;
use super::*;
use crate::alloc_counter::allocator_events;

fn target(control: u16) -> ControlTarget {
    ControlTarget {
        generation: 3,
        module_idx: 1,
        control: ControlIndex(control),
    }
}

fn request(value: f32) -> Request {
    Request::new(target(0), RequestValue::Value(RtValue::F32(value)))
}

/// Pushes until the queue refuses, returning how many it took.
fn fill(producer: &QueueProducer<u32>) -> usize {
    (0..).take_while(|&i| producer.try_push(i).is_ok()).count()
}

#[test]
fn pops_in_push_order() {
    let (producer, mut consumer) = bounded(4);
    assert_eq!(consumer.pop(), None);
    for i in 0..3 {
        producer.try_push(i).unwrap();
    }
    assert_eq!(consumer.pop(), Some(0));
    producer.try_push(3).unwrap();
    producer.try_push(4).unwrap();
    let rest: Vec<_> = std::iter::from_fn(|| consumer.pop()).collect();
    assert_eq!(rest, [1, 2, 3, 4]);
}

#[test]
fn capacity_rounds_up_to_a_power_of_two_of_at_least_two() {
    for (asked, held) in [(0, 2), (1, 2), (2, 2), (3, 4), (4, 4), (5, 8), (1000, 1024)] {
        let (producer, _consumer) = bounded(asked);
        assert_eq!(fill(&producer), held, "capacity {asked}");
    }
}

#[test]
fn a_full_queue_hands_the_value_back() {
    let (producer, mut consumer) = bounded(2);
    assert_eq!(fill(&producer), 2);
    assert_eq!(producer.try_push(7), Err(7));
    assert_eq!(consumer.pop(), Some(0));
    assert_eq!(producer.try_push(7), Ok(()));
    assert_eq!(producer.try_push(8), Err(8));
}

#[test]
fn positions_wrap_past_usize_max() {
    let (producer, mut consumer) = bounded_from(4, usize::MAX - 5);
    for i in 0..40u32 {
        producer.try_push(i).unwrap();
        if i % 3 == 0 {
            producer.try_push(1000 + i).unwrap();
            assert_eq!(consumer.pop(), Some(i));
            assert_eq!(consumer.pop(), Some(1000 + i));
        } else {
            assert_eq!(consumer.pop(), Some(i));
        }
    }
    assert_eq!(fill(&producer), 4, "the wrapped queue still holds 4");
    assert_eq!(producer.try_push(9), Err(9));
}

/// Counts its drops.
struct Tracked(Arc<AtomicUsize>);

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn dropping_the_queue_drops_unpopped_items_once() {
    for (start, consumer_first) in [(0, true), (0, false), (usize::MAX - 2, true)] {
        let drops = Arc::new(AtomicUsize::new(0));
        let (producer, mut consumer) = bounded_from(4, start);
        // Lap the slots once so the full/free test sees reused slots.
        for _ in 0..5 {
            producer.try_push(Tracked(drops.clone())).ok().unwrap();
            drop(consumer.pop());
        }
        assert_eq!(drops.load(Ordering::Relaxed), 5);
        for _ in 0..3 {
            producer.try_push(Tracked(drops.clone())).ok().unwrap();
        }
        drop(consumer.pop());
        assert_eq!(drops.load(Ordering::Relaxed), 6);
        if consumer_first {
            drop(consumer);
            assert_eq!(
                drops.load(Ordering::Relaxed),
                6,
                "the producer keeps the queue"
            );
            drop(producer);
        } else {
            drop(producer);
            drop(consumer);
        }
        assert_eq!(drops.load(Ordering::Relaxed), 8, "start {start}");
    }
}

#[test]
fn a_full_channel_returns_the_request_and_counts_the_overflow() {
    let (sender, mut consumer) = request_channel(2);
    let mut cursor = EventCursor::new();
    let first = sender.submit(request(1.0)).unwrap();
    let second = sender.clone().submit(request(2.0)).unwrap();
    let QueueFull(refused) = sender.submit(request(3.0)).unwrap_err();
    assert_eq!(refused.value, RequestValue::Value(RtValue::F32(3.0)));
    assert!(refused.id != first && refused.id != second);
    assert!(sender.submit(request(4.0)).is_err());
    assert_eq!(cursor.take(sender.overflows()), 2);

    let popped = consumer.pop().unwrap();
    assert_eq!((popped.id, popped.target), (first, target(0)));
    assert_eq!(consumer.pop().unwrap().id, second);
    assert!(sender.submit(request(5.0)).is_ok());
    assert_eq!(cursor.take(sender.overflows()), 0);
}

#[test]
fn request_ids_are_unique_across_threads() {
    let (sender, mut consumer) = request_channel(256);
    let submitters: Vec<_> = (0..4)
        .map(|_| {
            let sender = sender.clone();
            thread::spawn(move || {
                (0..50)
                    .map(|i| sender.submit(request(i as f32)).unwrap())
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut ids = HashSet::new();
    for submitter in submitters {
        for id in submitter.join().unwrap() {
            assert!(id != RequestId(0) && ids.insert(id), "duplicate {id:?}");
        }
    }
    let popped: HashSet<_> = std::iter::from_fn(|| consumer.pop())
        .map(|request| request.id)
        .collect();
    assert_eq!(popped, ids);
}

#[test]
fn concurrent_producers_keep_their_own_order() {
    let (producer, mut consumer) = bounded::<(u32, u32)>(8);
    let producers: Vec<_> = (0..3)
        .map(|p| {
            let producer = producer.clone();
            thread::spawn(move || {
                for i in 0..2000 {
                    while producer.try_push((p, i)).is_err() {
                        thread::yield_now();
                    }
                }
            })
        })
        .collect();
    let mut next = [0u32; 3];
    while next.iter().any(|&n| n < 2000) {
        match consumer.pop() {
            Some((p, i)) => {
                assert_eq!(i, next[p as usize], "producer {p} out of order");
                next[p as usize] += 1;
            }
            None => thread::yield_now(),
        }
    }
    for producer in producers {
        producer.join().unwrap();
    }
    assert_eq!(consumer.pop(), None);
}

#[test]
fn audio_side_paths_never_allocate_or_free() {
    const N: usize = 64;
    let (producer, mut consumer) = bounded::<Request>(N / 2);
    let (sender, mut requests) = request_channel(N / 2);
    let (sum, allocs, frees) = allocator_events(|| {
        let mut sum = 0.0;
        for i in 0..N {
            // More pushes than room: both the success and the full path.
            let refused = producer.try_push(request(i as f32)).is_err();
            let full = sender.submit(request(i as f32)).is_err();
            assert_eq!((refused, full), (i >= N / 2, i >= N / 2));
        }
        for _ in 0..N {
            for popped in [consumer.pop(), requests.pop()].into_iter().flatten() {
                if let RequestValue::Value(RtValue::F32(value)) = popped.value {
                    sum += value;
                }
            }
        }
        sum
    });
    let expected = (0..N / 2).sum::<usize>() as f32 * 2.0;
    assert_eq!((sum, allocs, frees), (expected, 0, 0));
}

#[test]
fn event_cursors_count_across_the_wrap() {
    let counter = EventCounter::starting_at(u32::MAX - 1);
    let mut cursor = EventCursor::new();
    assert_eq!(cursor.take(&counter), u32::MAX - 1);
    for _ in 0..3 {
        counter.record();
    }
    assert_eq!(cursor.take(&counter), 3);
    assert_eq!(cursor.take(&counter), 0);
}
