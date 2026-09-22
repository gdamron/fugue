use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Samples the tap holds before the analyser must catch up: about 0.7 s at
/// 48 kHz, far more than the few milliseconds between analysis passes.
const CAPACITY: usize = 32_768;

/// A mono tap of the master output, shared between the audio thread and an
/// off-thread analyser.
///
/// The audio thread is the only writer and the analyser the only reader. The
/// write path allocates nothing, takes no locks, and does no analysis: it sums
/// the two channels into a ring of samples and publishes one atomic index.
/// Samples are held as raw `f32` bits in atomics, so the handover needs no
/// mutex and no `unsafe`. When no one is listening the tap is disabled and the
/// write path returns after a single relaxed load.
///
/// If the analyser falls behind, the writer drops the samples that do not fit
/// and counts them rather than blocking or overwriting unread data. The count
/// lets the analyser advance its frame numbering across the hole, so dropped
/// audio shows up as a gap in time instead of silently compressing it.
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
    /// Next slot the reader will take.
    read: AtomicUsize,
    enabled: AtomicBool,
    dropped: AtomicU64,
}

impl Default for SpectrumTap {
    fn default() -> Self {
        Self::new()
    }
}

impl SpectrumTap {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(TapInner {
                samples: (0..CAPACITY).map(|_| AtomicU32::new(0)).collect(),
                write: AtomicUsize::new(0),
                read: AtomicUsize::new(0),
                enabled: AtomicBool::new(false),
                dropped: AtomicU64::new(0),
            }),
        }
    }

    /// Whether anything is currently analysing this tap.
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    /// Starts or stops collection. Disabling also discards what is buffered,
    /// so a later stream starts from live audio rather than stale samples.
    pub fn set_enabled(&self, enabled: bool) {
        if enabled {
            self.inner.enabled.store(true, Ordering::Relaxed);
        } else {
            self.inner.enabled.store(false, Ordering::Relaxed);
            let write = self.inner.write.load(Ordering::Acquire);
            self.inner.read.store(write, Ordering::Release);
            self.inner.dropped.store(0, Ordering::Relaxed);
        }
    }

    /// Samples dropped because the analyser fell behind, since the count was
    /// last taken.
    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// Reads and clears the dropped-sample count.
    pub fn take_dropped(&self) -> u64 {
        self.inner.dropped.swap(0, Ordering::Relaxed)
    }

    /// Audio thread: fold one block of output into the tap as mono.
    ///
    /// Allocation-free and lock-free. Returns immediately when disabled, so a
    /// running invention nobody is watching pays one relaxed load per block.
    #[inline]
    pub(crate) fn observe_block(&self, left: &[f32], right: &[f32], frames: usize) {
        if !self.is_enabled() {
            return;
        }
        let inner = &self.inner;
        let frames = frames.min(left.len()).min(right.len());
        let read = inner.read.load(Ordering::Acquire);
        let mut write = inner.write.load(Ordering::Relaxed);

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
        }
        inner.write.store(write, Ordering::Release);
        if dropped > 0 {
            inner.dropped.fetch_add(dropped, Ordering::Relaxed);
        }
    }

    /// Analyser: number of samples waiting to be read.
    pub fn available(&self) -> usize {
        let write = self.inner.write.load(Ordering::Acquire);
        let read = self.inner.read.load(Ordering::Relaxed);
        (write + CAPACITY - read) % CAPACITY
    }

    /// Analyser: copy up to `out.len()` samples out, returning how many.
    pub fn read_samples(&self, out: &mut [f32]) -> usize {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_nothing_until_enabled() {
        let tap = SpectrumTap::new();
        let values = [0.5; 64];
        tap.observe_block(&values, &values, 64);
        assert_eq!(tap.available(), 0);
    }

    #[test]
    fn sums_channels_to_mono() {
        let tap = SpectrumTap::new();
        tap.set_enabled(true);
        tap.observe_block(&[1.0, 0.0], &[0.0, 0.5], 2);
        let mut out = [0.0; 2];
        assert_eq!(tap.read_samples(&mut out), 2);
        assert_eq!(out, [0.5, 0.25]);
    }

    #[test]
    fn hands_samples_over_in_order_across_the_wrap() {
        let tap = SpectrumTap::new();
        tap.set_enabled(true);
        let mut out = vec![0.0; 1000];
        // Push far more than the ring holds, draining as we go.
        for round in 0..60u32 {
            let values: Vec<f32> = (0..1000).map(|i| (round * 1000 + i) as f32).collect();
            tap.observe_block(&values, &values, values.len());
            let count = tap.read_samples(&mut out);
            assert_eq!(count, 1000, "round {round} handed over {count}");
            assert_eq!(&out[..count], &values[..]);
        }
        assert_eq!(tap.dropped(), 0);
    }

    #[test]
    fn drops_and_counts_when_the_reader_falls_behind() {
        let tap = SpectrumTap::new();
        tap.set_enabled(true);
        let values = vec![0.25; CAPACITY];
        tap.observe_block(&values, &values, values.len());
        tap.observe_block(&values, &values, values.len());

        // One slot is reserved to tell full from empty.
        assert_eq!(tap.available(), CAPACITY - 1);
        assert_eq!(tap.dropped(), CAPACITY as u64 + 1);
        assert_eq!(tap.take_dropped(), CAPACITY as u64 + 1);
        assert_eq!(tap.dropped(), 0);
    }

    #[test]
    fn keeps_the_oldest_unread_samples_when_it_overflows() {
        let tap = SpectrumTap::new();
        tap.set_enabled(true);
        let first: Vec<f32> = (0..CAPACITY).map(|i| i as f32).collect();
        tap.observe_block(&first, &first, first.len());
        let late = vec![-1.0; 512];
        tap.observe_block(&late, &late, late.len());

        let mut out = vec![0.0; 4];
        tap.read_samples(&mut out);
        assert_eq!(out, [0.0, 1.0, 2.0, 3.0], "unread audio was overwritten");
    }

    #[test]
    fn disabling_discards_what_is_buffered() {
        let tap = SpectrumTap::new();
        tap.set_enabled(true);
        let values = [0.5; 128];
        tap.observe_block(&values, &values, 128);
        assert_eq!(tap.available(), 128);

        tap.set_enabled(false);
        assert_eq!(tap.available(), 0);
        tap.set_enabled(true);
        tap.observe_block(&values, &values, 8);
        assert_eq!(tap.available(), 8);
    }
}
