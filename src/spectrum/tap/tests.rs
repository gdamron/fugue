//! Tests for the spectrum tap.

use super::*;

fn tap_and_reader() -> (SpectrumTap, SpectrumReader) {
    let tap = SpectrumTap::new();
    let mut reader = tap.take_reader().expect("a fresh tap has its reader");
    reader.start();
    (tap, reader)
}

fn feed(tap: &SpectrumTap, samples: usize) {
    let block = vec![0.25; samples];
    tap.observe_block(&block, &block, block.len());
}

/// Reads everything waiting, returning how much there was.
fn drain(reader: &mut SpectrumReader) -> usize {
    let mut out = vec![0.0; CAPACITY];
    reader.read_samples(&mut out)
}

#[test]
fn collects_nothing_until_started() {
    let tap = SpectrumTap::new();
    let reader = tap.take_reader().unwrap();
    feed(&tap, 64);
    assert_eq!(reader.available(), 0);
}

#[test]
fn hands_out_its_reader_only_once() {
    let tap = SpectrumTap::new();
    let reader = tap.take_reader().expect("first take succeeds");
    assert!(
        tap.take_reader().is_none(),
        "a second consumer would corrupt the read index"
    );

    drop(reader);
    assert!(
        tap.take_reader().is_some(),
        "dropping the reader returns the reading end"
    );
}

#[test]
fn dropping_the_reader_stops_collection() {
    let (tap, reader) = tap_and_reader();
    assert!(tap.is_enabled());
    drop(reader);
    assert!(
        !tap.is_enabled(),
        "an abandoned stream must not leave the audio thread collecting"
    );
}

#[test]
fn sums_channels_to_mono() {
    let (tap, mut reader) = tap_and_reader();
    tap.observe_block(&[1.0, 0.0], &[0.0, 0.5], 2);
    let mut out = [0.0; 2];
    assert_eq!(reader.read_samples(&mut out), 2);
    assert_eq!(out, [0.5, 0.25]);
}

#[test]
fn hands_samples_over_in_order_across_the_wrap() {
    let (tap, mut reader) = tap_and_reader();
    let mut out = vec![0.0; 1000];
    // Push far more than the ring holds, draining as we go.
    for round in 0..60u32 {
        let values: Vec<f32> = (0..1000).map(|i| (round * 1000 + i) as f32).collect();
        tap.observe_block(&values, &values, values.len());
        let count = reader.read_samples(&mut out);
        assert_eq!(count, 1000, "round {round} handed over {count}");
        assert_eq!(&out[..count], &values[..]);
    }
    assert_eq!(reader.dropped_total(), 0);
    assert_eq!(reader.position(), 60_000);
    assert!(reader.take_hole().is_none());
}

#[test]
fn keeps_the_oldest_unread_samples_when_it_overflows() {
    let (tap, mut reader) = tap_and_reader();
    let first: Vec<f32> = (0..CAPACITY).map(|i| i as f32).collect();
    tap.observe_block(&first, &first, first.len());
    let late = vec![-1.0; 512];
    tap.observe_block(&late, &late, late.len());

    let mut out = vec![0.0; 4];
    reader.read_samples(&mut out);
    assert_eq!(out, [0.0, 1.0, 2.0, 3.0], "unread audio was overwritten");
}

#[test]
fn publishes_a_loss_only_once_it_has_ended() {
    let (tap, mut reader) = tap_and_reader();
    feed(&tap, CAPACITY);
    feed(&tap, 1_000);

    // Still being dropped, so its length is not final: nothing to report yet.
    assert!(reader.take_hole().is_none());

    // Make room; the next block ends the loss and publishes it first.
    drain(&mut reader);
    feed(&tap, 64);
    let accepted_first = (CAPACITY - 1) as u64;
    let (at, len) = reader.take_hole().expect("the loss has ended");
    assert_eq!(at, accepted_first, "it sits after the audio already taken");
    assert_eq!(
        len,
        1 + 1_000,
        "one sample of the first block, then all of the second"
    );
    assert_eq!(reader.available(), 64, "audio after the loss arrives too");
    assert!(reader.take_hole().is_none(), "a loss is reported once");
}

#[test]
fn announces_a_loss_before_any_audio_after_it_is_readable() {
    let (tap, mut reader) = tap_and_reader();
    feed(&tap, CAPACITY);
    feed(&tap, 500);
    drain(&mut reader);
    feed(&tap, 64);

    // The reader looks at what is available, then for losses: the loss that
    // precedes the new audio must already be visible.
    assert_eq!(reader.available(), 64);
    let (at, _) = reader
        .take_hole()
        .expect("published before the audio after it");
    assert_eq!(
        at,
        reader.position(),
        "and it sits exactly where reading stopped"
    );
}

/// A second stream on the same tap counts positions from its own start, so
/// its losses land where they happened in its audio.
#[test]
fn positions_count_from_each_streams_start() {
    let tap = SpectrumTap::new();
    let mut first = tap.take_reader().unwrap();
    first.start();
    for _ in 0..50 {
        feed(&tap, 1_000);
        drain(&mut first);
    }
    assert_eq!(first.position(), 50_000);
    drop(first);

    let mut second = tap.take_reader().unwrap();
    second.start();
    assert_eq!(second.position(), 0, "a new stream starts at zero");
    feed(&tap, 2_000);
    drain(&mut second);
    feed(&tap, CAPACITY);
    feed(&tap, 700);
    drain(&mut second);
    feed(&tap, 1);

    let (at, len) = second.take_hole().expect("the second stream's loss");
    assert_eq!(
        at,
        2_000 + (CAPACITY - 1) as u64,
        "measured from the second stream's start, not the tap's"
    );
    assert_eq!(len, 1 + 700);
}

#[test]
fn ignores_losses_left_over_from_an_earlier_stream() {
    let tap = SpectrumTap::new();
    let mut first = tap.take_reader().unwrap();
    first.start();
    feed(&tap, CAPACITY);
    feed(&tap, 900); // lost, never published: the stream ends mid-loss
    drop(first);

    let mut second = tap.take_reader().unwrap();
    second.start();
    feed(&tap, 100);
    assert_eq!(second.available(), 100);
    assert!(
        second.take_hole().is_none(),
        "the first stream's loss is not the second stream's"
    );
}

#[test]
fn keeps_separate_losses_separate() {
    let (tap, mut reader) = tap_and_reader();
    // Two stalls with audio accepted between them, the reader reaching
    // neither loss in the meantime.
    feed(&tap, CAPACITY);
    feed(&tap, 300);
    let mut out = vec![0.0; 1_000];
    reader.read_samples(&mut out);
    feed(&tap, 2_000); // publishes the first loss, accepts 1,000, drops 1,000
    reader.read_samples(&mut out);
    feed(&tap, 10); // publishes the second

    let first = reader.take_hole().expect("the first loss");
    let second = reader.take_hole().expect("the second loss");
    assert_eq!(first, ((CAPACITY - 1) as u64, 1 + 300));
    assert_eq!(
        second,
        ((CAPACITY - 1) as u64 + 1_000, 1_000),
        "audio accepted between two losses keeps them apart"
    );
}

#[test]
fn keeps_every_lost_sample_when_the_loss_queue_is_full() {
    let (tap, mut reader) = tap_and_reader();
    let mut out = vec![0.0; 256];
    // More stalls than the queue holds, the reader never taking a loss.
    for _ in 0..HOLE_CAPACITY + 8 {
        feed(&tap, CAPACITY);
        reader.read_samples(&mut out);
    }
    // Now take them all, letting the writer publish what it held back.
    let mut lost = 0;
    for _ in 0..4 {
        while let Some((_, len)) = reader.take_hole() {
            lost += len;
        }
        reader.read_samples(&mut out);
        feed(&tap, 1);
    }
    while let Some((_, len)) = reader.take_hole() {
        lost += len;
    }
    assert_eq!(
        lost,
        reader.dropped_total(),
        "every dropped sample is accounted for in exactly one loss"
    );
}

#[test]
fn restarting_discards_what_is_buffered() {
    let (tap, mut reader) = tap_and_reader();
    feed(&tap, 128);
    assert_eq!(reader.available(), 128);

    reader.stop();
    reader.start();
    assert_eq!(reader.available(), 0);
    assert_eq!(reader.position(), 0);
    feed(&tap, 8);
    assert_eq!(reader.available(), 8);
}

/// The audio thread and the analyser really do run concurrently. Every sample
/// handed to the tap must come out exactly once — read, or counted in a loss —
/// and no loss may be announced after audio beyond it was readable.
#[test]
fn accounts_for_every_sample_across_threads() {
    use std::sync::atomic::AtomicBool;
    use std::thread;
    use std::time::Duration;

    let (tap, mut reader) = tap_and_reader();
    let finished = Arc::new(AtomicBool::new(false));
    let writer = {
        let tap = tap.clone();
        let finished = Arc::clone(&finished);
        thread::spawn(move || {
            let block = [0.1f32; 64];
            let mut fed = 0u64;
            for n in 0..40_000u32 {
                tap.observe_block(&block, &block, block.len());
                fed += 64;
                // Roughly real-time pacing, so reading and writing genuinely
                // overlap instead of the writer finishing first.
                if n.is_multiple_of(32) {
                    thread::sleep(Duration::from_micros(200));
                }
            }
            finished.store(true, Ordering::Release);
            fed
        })
    };

    let mut out = vec![0.0; 4_096];
    let mut read = 0u64;
    let mut lost = 0u64;
    let mut pending: Option<(u64, u64)> = None;
    let mut round = 0u64;
    let mut losses = 0u32;
    let mut read_while_writing = 0u64;
    loop {
        let done = finished.load(Ordering::Acquire);
        // The analyser's protocol: look at what is available, then for
        // losses, and never read past one.
        let available = reader.available() as u64;
        if pending.is_none() {
            pending = reader.take_hole();
        }
        if let Some((at, len)) = pending {
            assert!(
                reader.position() <= at,
                "loss at {at} announced after reading reached {}",
                reader.position()
            );
            if reader.position() == at {
                lost += len;
                losses += 1;
                pending = None;
                continue;
            }
        }
        let limit = pending.map_or(available, |(at, _)| available.min(at - reader.position()));
        let count = limit.min(out.len() as u64) as usize;
        let got = reader.read_samples(&mut out[..count]) as u64;
        read += got;
        if !done {
            read_while_writing += got;
        }

        round += 1;
        if round.is_multiple_of(50) {
            // Stall now and then, long enough to overrun the ring.
            thread::sleep(Duration::from_millis(40));
        } else {
            thread::sleep(Duration::from_micros(100));
        }
        if done && available == 0 && pending.is_none() {
            break;
        }
    }
    let fed = writer.join().unwrap();

    // A loss still in progress when the writer stopped is published by the
    // next block; send one and take it.
    let spare = [0.0f32; 1];
    let (mut tail_read, mut tail_lost) = (0u64, 0u64);
    tap.observe_block(&spare, &spare, 1);
    if let Some((_, len)) = reader.take_hole() {
        tail_lost += len;
    }
    tail_read += reader.read_samples(&mut out) as u64;

    assert!(
        losses >= 3,
        "expected several separate losses, saw {losses}; the queue is not being exercised"
    );
    // Most audio is lost during the stalls by design, so measure overlap
    // against what was read, not what was fed.
    assert!(
        read_while_writing > read / 2 && read_while_writing > 100_000,
        "only {read_while_writing} of {read} samples were read while writing; not concurrent"
    );
    assert_eq!(
        read + lost + tail_read + tail_lost,
        fed + 1,
        "every sample is read or counted in a loss, exactly once"
    );
}
