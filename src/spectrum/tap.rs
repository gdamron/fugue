use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Samples the tap holds before the analyser must catch up: about 0.7 s at
/// 48 kHz, far more than the few milliseconds between analysis passes.
const CAPACITY: usize = 32_768;

/// `hole_at` when no hole is waiting to be accounted for.
const NO_HOLE: u64 = u64::MAX;

/// The audio thread's end of a mono tap of the master output.
///
/// The write path allocates nothing, takes no locks, and does no analysis: it
/// sums the two channels into a ring of samples and publishes one index.
/// Samples are held as raw `f32` bits in atomics, so the handover needs no
/// mutex and no `unsafe`. When no one is listening the tap is disabled and the
/// write path returns after a single relaxed load.
///
/// The reading end is a [`SpectrumReader`], which exists at most once per tap
/// (see [`take_reader`](Self::take_reader)). That is what makes the index
/// handover sound: one writer, one reader, neither doing a read-modify-write
/// on the other's index.
#[derive(Clone)]
pub struct SpectrumTap {
    inner: Arc<TapInner>,
}

struct TapInner {
    /// Ring storage, one `f32`'s bits per slot.
    samples: Vec<AtomicU32>,
    /// Next slot the writer will fill. Published with `Release` so a reader
    /// that sees it also sees the samples written before it.
    write: AtomicUsize,
    /// Next slot the reader will take. Only the reader stores to this.
    read: AtomicUsize,
    enabled: AtomicBool,
    /// Samples accepted into the ring since collection began. The reader
    /// consumes them in order, so its own running count is directly
    /// comparable with `hole_at`.
    accepted: AtomicU64,
    /// Where the next unaccounted hole sits, counted in accepted samples, or
    /// [`NO_HOLE`]. Positions the loss in the stream: the drop happened at the
    /// writing end, while the reader may still be most of a ring behind.
    hole_at: AtomicU64,
    /// Samples lost at `hole_at`.
    hole_len: AtomicU64,
    /// Every sample ever dropped, for diagnostics.
    dropped_total: AtomicU64,
    /// Whether the single reader has been handed out.
    reader_taken: AtomicBool,
}

impl SpectrumTap {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(TapInner {
                samples: (0..CAPACITY).map(|_| AtomicU32::new(0)).collect(),
                write: AtomicUsize::new(0),
                read: AtomicUsize::new(0),
                enabled: AtomicBool::new(false),
                accepted: AtomicU64::new(0),
                hole_at: AtomicU64::new(NO_HOLE),
                hole_len: AtomicU64::new(0),
                dropped_total: AtomicU64::new(0),
                reader_taken: AtomicBool::new(false),
            }),
        }
    }

    /// Takes the tap's one reading end, or `None` when something already
    /// holds it. Dropping the reader gives it back and stops collection.
    pub fn take_reader(&self) -> Option<SpectrumReader> {
        self.inner
            .reader_taken
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| SpectrumReader {
                inner: Arc::clone(&self.inner),
            })
    }

    /// Whether anything is currently analysing this tap.
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    /// Every sample ever dropped because the analyser fell behind.
    pub fn dropped_total(&self) -> u64 {
        self.inner.dropped_total.load(Ordering::Relaxed)
    }

    /// Audio thread: fold one block of output into the tap as mono.
    ///
    /// Allocation-free and lock-free. Returns immediately when disabled, so a
    /// running invention nobody is watching pays one relaxed load per block.
    ///
    /// Channels are summed and halved; a signal panned hard to one side
    /// therefore reads 6 dB below its channel peak, and an out-of-phase pair
    /// cancels. That is what `source` names as a mono analysis.
    #[inline]
    pub(crate) fn observe_block(&self, left: &[f32], right: &[f32], frames: usize) {
        if !self.is_enabled() {
            return;
        }
        let inner = &self.inner;
        let frames = frames.min(left.len()).min(right.len());
        let read = inner.read.load(Ordering::Acquire);
        let mut write = inner.write.load(Ordering::Relaxed);
        let mut accepted = inner.accepted.load(Ordering::Relaxed);

        let mut dropped = 0u64;
        for i in 0..frames {
            let next = if write + 1 == CAPACITY { 0 } else { write + 1 };
            if next == read {
                // Full: the analyser has not caught up. Drop the rest of the
                // block rather than overwrite samples it has not read.
                dropped = (frames - i) as u64;
                break;
            }
            let mono = 0.5 * (left[i] + right[i]);
            inner.samples[write].store(mono.to_bits(), Ordering::Relaxed);
            write = next;
            accepted += 1;
        }
        inner.write.store(write, Ordering::Release);
        inner.accepted.store(accepted, Ordering::Release);
        if dropped > 0 {
            inner.record_hole(accepted, dropped);
        }
    }
}

impl TapInner {
    /// Writer only: notes that `len` samples were lost after `at` accepted
    /// samples, so the reader can place the gap where it actually happened.
    fn record_hole(&self, at: u64, len: u64) {
        self.dropped_total.fetch_add(len, Ordering::Relaxed);
        if self.hole_at.load(Ordering::Acquire) == NO_HOLE {
            self.hole_len.store(len, Ordering::Relaxed);
            // Publish the position last: a reader that sees it also sees the
            // length that belongs to it.
            self.hole_at.store(at, Ordering::Release);
        } else {
            // The reader has not reached the previous hole yet, which means it
            // is more than a ring behind. Merge rather than queue: the two
            // gaps are reported as one at the earlier position, which can only
            // under-state how far apart they were.
            self.hole_len.fetch_add(len, Ordering::Relaxed);
        }
    }
}

/// The analyser's end of a [`SpectrumTap`]: the only consumer, and the only
/// thing that stores to the read index.
///
/// Dropping it stops collection and returns the reading end to the tap, so a
/// stream that ends by an early return or a panic cannot leave the audio
/// thread filling a ring nobody drains.
pub struct SpectrumReader {
    inner: Arc<TapInner>,
}

impl SpectrumReader {
    /// Starts collection, discarding anything buffered so analysis begins from
    /// live audio rather than stale samples.
    pub fn start(&mut self) {
        self.discard_buffered();
        self.inner.enabled.store(true, Ordering::Relaxed);
    }

    /// Stops collection.
    pub fn stop(&mut self) {
        self.inner.enabled.store(false, Ordering::Relaxed);
    }

    /// Whether collection is running.
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    /// Samples waiting to be read.
    pub fn available(&self) -> usize {
        let write = self.inner.write.load(Ordering::Acquire);
        let read = self.inner.read.load(Ordering::Relaxed);
        (write + CAPACITY - read) % CAPACITY
    }

    /// Copies up to `out.len()` samples out, returning how many.
    pub fn read_samples(&mut self, out: &mut [f32]) -> usize {
        let write = self.inner.write.load(Ordering::Acquire);
        let mut read = self.inner.read.load(Ordering::Relaxed);
        let available = (write + CAPACITY - read) % CAPACITY;
        let count = available.min(out.len());
        for value in out.iter_mut().take(count) {
            *value = f32::from_bits(self.inner.samples[read].load(Ordering::Relaxed));
            read = if read + 1 == CAPACITY { 0 } else { read + 1 };
        }
        self.inner.read.store(read, Ordering::Release);
        count
    }

    /// The next hole waiting to be accounted for, as the number of samples
    /// that had been accepted before it and the number lost there.
    ///
    /// The caller compares the position with its own running count of samples
    /// read, and applies the gap when it reaches that point — not when it
    /// first hears about it, which is up to a ring of good audio too early.
    pub fn take_hole(&mut self) -> Option<(u64, u64)> {
        if self.inner.hole_at.load(Ordering::Acquire) == NO_HOLE {
            return None;
        }
        let at = self.inner.hole_at.load(Ordering::Acquire);
        let len = self.inner.hole_len.swap(0, Ordering::AcqRel);
        self.inner.hole_at.store(NO_HOLE, Ordering::Release);
        // A drop landing in the instant between those two stores is recorded
        // against the next hole instead of this one; it is still counted in
        // `dropped_total`, and costs only that burst's positional accuracy.
        (len > 0).then_some((at, len))
    }

    /// Every sample ever dropped because analysis fell behind.
    pub fn dropped_total(&self) -> u64 {
        self.inner.dropped_total.load(Ordering::Relaxed)
    }

    /// Drops everything buffered, along with any hole recorded in it.
    fn discard_buffered(&mut self) {
        let write = self.inner.write.load(Ordering::Acquire);
        self.inner.read.store(write, Ordering::Release);
        self.inner.hole_at.store(NO_HOLE, Ordering::Release);
        self.inner.hole_len.store(0, Ordering::Relaxed);
    }
}

impl Drop for SpectrumReader {
    fn drop(&mut self) {
        self.stop();
        self.discard_buffered();
        self.inner.reader_taken.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tap_and_reader() -> (SpectrumTap, SpectrumReader) {
        let tap = SpectrumTap::new();
        let mut reader = tap.take_reader().expect("a fresh tap has its reader");
        reader.start();
        (tap, reader)
    }

    #[test]
    fn collects_nothing_until_started() {
        let tap = SpectrumTap::new();
        let reader = tap.take_reader().unwrap();
        let values = [0.5; 64];
        tap.observe_block(&values, &values, 64);
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
        assert!(reader.take_hole().is_none());
    }

    #[test]
    fn records_where_a_hole_happened_not_just_how_big_it_was() {
        let (tap, mut reader) = tap_and_reader();
        let values = vec![0.25; CAPACITY];
        tap.observe_block(&values, &values, values.len());
        tap.observe_block(&values, &values, values.len());

        // One slot is reserved to tell full from empty.
        let accepted = (CAPACITY - 1) as u64;
        assert_eq!(reader.available(), CAPACITY - 1);
        assert_eq!(reader.dropped_total(), CAPACITY as u64 + 1);

        let (at, len) = reader.take_hole().expect("a hole was recorded");
        assert_eq!(at, accepted, "the hole sits after the audio already taken");
        assert_eq!(len, CAPACITY as u64 + 1);
        assert!(reader.take_hole().is_none(), "a hole is reported once");
    }

    #[test]
    fn merges_a_second_hole_the_reader_has_not_reached() {
        let (tap, mut reader) = tap_and_reader();
        let values = vec![0.25; CAPACITY];
        tap.observe_block(&values, &values, values.len());
        tap.observe_block(&values, &values, values.len());
        tap.observe_block(&values, &values, values.len());

        let (_, len) = reader.take_hole().unwrap();
        assert_eq!(
            len,
            reader.dropped_total(),
            "every dropped sample is accounted for somewhere"
        );
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
    fn restarting_discards_what_is_buffered() {
        let (tap, mut reader) = tap_and_reader();
        let values = [0.5; 128];
        tap.observe_block(&values, &values, 128);
        assert_eq!(reader.available(), 128);

        reader.stop();
        reader.start();
        assert_eq!(reader.available(), 0);
        tap.observe_block(&values, &values, 8);
        assert_eq!(reader.available(), 8);
    }
}
