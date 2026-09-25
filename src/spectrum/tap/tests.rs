//! Tests for the spectrum tap.
//!
//! Most feed samples whose value encodes their own index in the tap's
//! lifetime, so any sample read out proves where it came from, and a loss is
//! checked by the indices it skipped rather than by counts alone.

use super::*;

/// The sample value that stands for index `n`: `n / 2^24`, which stays below
/// the tap's ±1 clamp and exact in `f32` for every index these tests reach.
fn number(n: u64) -> f32 {
    n as f32 / (1u32 << 24) as f32
}

/// Feeds `count` samples numbered from `first`, returning the next number.
fn feed_numbered(tap: &SpectrumTap, first: u64, count: usize) -> u64 {
    let block: Vec<f32> = (0..count as u64).map(|i| number(first + i)).collect();
    tap.observe_block(&block, &block, block.len());
    first + count as u64
}

fn feed(tap: &SpectrumTap, samples: usize) {
    let block = vec![0.25; samples];
    tap.observe_block(&block, &block, block.len());
}

#[test]
fn collects_nothing_without_a_reader() {
    let tap = SpectrumTap::new();
    feed(&tap, 64);
    assert!(!tap.is_collecting());
    assert_eq!(tap.inner.written.load(Ordering::Relaxed), 0);
}

#[test]
fn collects_while_any_reader_lives() {
    let tap = SpectrumTap::new();
    let first = tap.reader();
    let second = tap.reader();
    assert!(tap.is_collecting());
    drop(first);
    assert!(tap.is_collecting(), "one reader is still listening");
    drop(second);
    assert!(
        !tap.is_collecting(),
        "an abandoned stream must not leave the audio thread collecting"
    );
}

#[test]
fn sums_channels_to_mono() {
    let tap = SpectrumTap::new();
    let mut reader = tap.reader();
    tap.observe_block(&[1.0, 0.0], &[0.0, 0.5], 2);
    let mut out = [0.0; 2];
    assert_eq!(reader.read(&mut out), TapRead { lost: 0, count: 2 });
    assert_eq!(out, [0.5, 0.25]);
}

/// The tap hears what the DAC sends: each channel clipped to ±1 before
/// the channels are folded together.
#[test]
fn clamps_each_channel_as_the_dac_does() {
    let tap = SpectrumTap::new();
    let mut reader = tap.reader();
    tap.observe_block(&[3.0, -3.0, 2.0, f32::INFINITY], &[0.5, 0.0, -2.0, 0.0], 4);
    let mut out = [0.0; 4];
    assert_eq!(reader.read(&mut out).count, 4);
    assert_eq!(out, [0.75, -0.5, 0.0, 0.5]);
}

#[test]
fn a_reader_starts_at_the_live_edge() {
    let tap = SpectrumTap::new();
    let _earlier = tap.reader();
    feed(&tap, 500);

    let mut reader = tap.reader();
    assert_eq!(reader.position(), 0);
    let mut out = [0.0; 16];
    assert_eq!(reader.read(&mut out).count, 0, "nothing from before it");
    feed(&tap, 8);
    assert_eq!(reader.read(&mut out), TapRead { lost: 0, count: 8 });
}

#[test]
fn hands_samples_over_in_order_across_the_wrap() {
    let tap = SpectrumTap::new();
    let mut reader = tap.reader();
    let mut out = vec![0.0; 1_000];
    let mut next = 0;
    // Push far more than the ring holds, draining as we go.
    for round in 0..60 {
        let first = next;
        next = feed_numbered(&tap, next, 1_000);
        let read = reader.read(&mut out);
        assert_eq!(
            read,
            TapRead {
                lost: 0,
                count: 1_000
            },
            "round {round}"
        );
        assert!(out.iter().zip(first..).all(|(v, i)| *v == number(i)));
    }
    assert_eq!(reader.lost_total(), 0);
    assert_eq!(reader.position(), 60_000);
}

#[test]
fn a_stalled_reader_resumes_on_the_newest_audio_and_counts_the_rest() {
    let tap = SpectrumTap::new();
    let mut reader = tap.reader();
    let mut out = vec![0.0; 100];
    let next = feed_numbered(&tap, 0, 100);
    reader.read(&mut out);

    // The reader stalls while three ring-fulls go by.
    let next = feed_numbered(&tap, next, 3 * CAPACITY + 40);
    let mut out = vec![0.0; CAPACITY];
    let read = reader.read(&mut out);
    let oldest_kept = next - CAPACITY as u64;
    assert_eq!(read.lost, oldest_kept - 100, "everything overwritten");
    assert_eq!(read.count, CAPACITY, "the whole ring is still readable");
    assert_eq!(
        out[0],
        number(oldest_kept),
        "resumes at the oldest survivor"
    );
    assert_eq!(out[CAPACITY - 1], number(next - 1));
    assert_eq!(reader.position(), next, "lost audio still moves time on");
    assert_eq!(reader.lost_total(), read.lost);
}

#[test]
fn a_short_read_leaves_the_rest_for_the_next() {
    let tap = SpectrumTap::new();
    let mut reader = tap.reader();
    feed_numbered(&tap, 0, 10);
    let mut out = [0.0; 4];
    assert_eq!(reader.read(&mut out).count, 4);
    assert_eq!(out, [0, 1, 2, 3].map(number));
    assert_eq!(reader.read(&mut out).count, 4);
    assert_eq!(out, [4, 5, 6, 7].map(number));
    assert_eq!(reader.read(&mut out).count, 2);
    assert_eq!(reader.position(), 10);
}

#[test]
fn readers_do_not_disturb_each_other() {
    let tap = SpectrumTap::new();
    let mut fast = tap.reader();
    let mut slow = tap.reader();
    let mut out = vec![0.0; 1_000];
    let mut next = 0;
    for _ in 0..40 {
        next = feed_numbered(&tap, next, 1_000);
        assert_eq!(fast.read(&mut out).lost, 0);
    }
    let read = slow.read(&mut out);
    assert_eq!(read.lost, next - CAPACITY as u64);
    assert_eq!(out[0], number(next - CAPACITY as u64));
    assert_eq!(fast.lost_total(), 0, "one reader's stall is its own");
}

/// A writer lapping the reader mid-copy is too quick to catch in a race, so
/// stage it: a claim published for samples not yet written condemns the
/// oldest slots they will land in, and the reader must not trust them.
#[test]
fn samples_being_overwritten_mid_read_count_as_lost() {
    let tap = SpectrumTap::new();
    let mut reader = tap.reader();
    let next = feed_numbered(&tap, 0, CAPACITY);
    // The writer has claimed the next 10 slots and is part-way through them.
    tap.inner.claimed.store(next + 10, Ordering::Relaxed);

    let mut out = vec![0.0; CAPACITY];
    let read = reader.read(&mut out);
    assert_eq!(
        read,
        TapRead {
            lost: 10,
            count: CAPACITY - 10
        }
    );
    assert_eq!(
        out[0],
        number(10),
        "the first trustworthy sample comes first"
    );
    assert_eq!(reader.position(), next);
}

/// Only exercised when a block of audio is itself longer than the ring, which
/// the engine never produces, but the tap must still not misplace samples.
#[test]
fn a_block_longer_than_the_ring_keeps_its_newest_samples() {
    let tap = SpectrumTap::new();
    let mut reader = tap.reader();
    let next = feed_numbered(&tap, 0, CAPACITY + 123);
    let mut out = vec![0.0; CAPACITY];
    let read = reader.read(&mut out);
    assert_eq!(
        read,
        TapRead {
            lost: 123,
            count: CAPACITY
        }
    );
    assert_eq!(out[0], number(123));
    assert_eq!(out[CAPACITY - 1], number(next - 1));
}

/// The audio thread and an analyser really do run concurrently, and the
/// analyser stalls long enough to be lapped. Every sample read must be the
/// right one for its position, and read plus lost must account for every
/// sample written.
#[test]
fn accounts_for_every_sample_across_threads() {
    use std::sync::atomic::AtomicBool;
    use std::thread;
    use std::time::Duration;

    let tap = SpectrumTap::new();
    let mut reader = tap.reader();
    let finished = Arc::new(AtomicBool::new(false));
    let writer = {
        let tap = tap.clone();
        let finished = Arc::clone(&finished);
        thread::spawn(move || {
            // Values stay exact as `f32` well past what this test writes.
            let mut next = 0u64;
            for n in 0..40_000u32 {
                next = feed_numbered(&tap, next, 64);
                // Roughly real-time pacing, so reading and writing genuinely
                // overlap instead of the writer finishing first.
                if n.is_multiple_of(32) {
                    thread::sleep(Duration::from_micros(200));
                }
            }
            finished.store(true, Ordering::Release);
            next
        })
    };

    let mut out = vec![0.0; 4_096];
    let (mut read_total, mut losses, mut read_while_writing) = (0u64, 0u32, 0u64);
    let mut round = 0u64;
    loop {
        let done = finished.load(Ordering::Acquire);
        let read = reader.read(&mut out);
        if read.lost > 0 {
            losses += 1;
        }
        // Every sample is the one that belongs at its position.
        let first = reader.position() - read.count as u64;
        for (offset, value) in out[..read.count].iter().enumerate() {
            assert_eq!(
                *value,
                number(first + offset as u64),
                "sample at {} is from somewhere else",
                first + offset as u64
            );
        }
        read_total += read.count as u64;
        if !done {
            read_while_writing += read.count as u64;
        }

        round += 1;
        if round.is_multiple_of(50) {
            // Stall now and then, long enough to be lapped.
            thread::sleep(Duration::from_millis(40));
        } else {
            thread::sleep(Duration::from_micros(100));
        }
        if done && read.count == 0 {
            break;
        }
    }
    let written = writer.join().unwrap();

    assert!(
        losses >= 3,
        "expected several separate losses, saw {losses}; overwrite is not being exercised"
    );
    assert!(
        read_while_writing > read_total / 2 && read_while_writing > 100_000,
        "only {read_while_writing} of {read_total} samples were read while writing; not concurrent"
    );
    assert_eq!(
        read_total + reader.lost_total(),
        written,
        "every sample is read or counted as lost, exactly once"
    );
}
