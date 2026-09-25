use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Samples the tap holds before the analyser must catch up: about 0.7 s at
/// 48 kHz, far more than the few milliseconds between analysis passes.
const CAPACITY: usize = 32_768;

/// Losses the writer can publish before the reader takes any. One is published
/// per stall, so this is only reached when the analyser stalls this many times
/// without reaching the first one — and even then no loss is miscounted: the
/// writer keeps dropping into the loss it has not yet published.
const HOLE_CAPACITY: usize = 32;

/// The audio thread's end of a mono tap of the master output.
///
/// The write path allocates nothing, takes no locks, and does no analysis: it
/// sums the two channels into a ring of samples and publishes one index.
/// Samples are held as raw `f32` bits in atomics, so the handover needs no
/// mutex and no `unsafe`. When no one is listening the tap is disabled and the
/// write path returns after a single relaxed load.
///
/// The reading end is a [`SpectrumReader`], which exists at most once at a
/// time (see [`take_reader`](Self::take_reader)). One writer and one reader,
/// each the only thread that stores to its own indices, is what makes every
/// handover here sound.
#[derive(Clone)]
pub struct SpectrumTap {
    inner: Arc<TapInner>,
}

struct TapInner {
    /// Ring storage, one `f32`'s bits per slot.
    samples: Vec<AtomicU32>,
    /// Next slot the writer will fill. Published with `Release` so a reader
    /// that sees it also sees the samples, and any loss, written before it.
    write: AtomicUsize,
    /// Next slot the reader will take. Only the reader stores to this.
    read: AtomicUsize,
    enabled: AtomicBool,
    /// Bumped each time a reader starts a stream. Losses stamped with an
    /// earlier value belong to a stream that no longer exists.
    generation: AtomicU64,
    /// Samples ever accepted into the ring. Writer only.
    accepted: AtomicU64,
    /// Samples ever taken past the read index, read or discarded. Reader only.
    /// Every accepted sample passes the read index exactly once, so this and
    /// `accepted` count in the same units, and a loss's position can be
    /// compared with the reader's without either side resetting anything.
    taken: AtomicU64,
    /// Published losses, oldest first: a single-producer, single-consumer
    /// queue of immutable records.
    holes: Vec<HoleRecord>,
    /// Next record the reader will take. Reader only.
    hole_head: AtomicUsize,
    /// Next record the writer will fill. Writer only.
    hole_tail: AtomicUsize,
    /// The loss currently being dropped, not yet published because it is
    /// still growing. Writer only; atomics only because `observe_block` takes
    /// `&self`.
    pending_at: AtomicU64,
    pending_len: AtomicU64,
    pending_generation: AtomicU64,
    /// Every sample ever dropped, for diagnostics.
    dropped_total: AtomicU64,
    /// Whether the single reader has been handed out.
    reader_taken: AtomicBool,
}

/// One published loss: `len` samples lost after `at` accepted samples.
/// Written once by the writer, then only read.
struct HoleRecord {
    at: AtomicU64,
    len: AtomicU64,
    generation: AtomicU64,
}

impl SpectrumTap {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(TapInner {
                samples: (0..CAPACITY).map(|_| AtomicU32::new(0)).collect(),
                write: AtomicUsize::new(0),
                read: AtomicUsize::new(0),
                enabled: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                accepted: AtomicU64::new(0),
                taken: AtomicU64::new(0),
                holes: (0..HOLE_CAPACITY)
                    .map(|_| HoleRecord {
                        at: AtomicU64::new(0),
                        len: AtomicU64::new(0),
                        generation: AtomicU64::new(0),
                    })
                    .collect(),
                hole_head: AtomicUsize::new(0),
                hole_tail: AtomicUsize::new(0),
                pending_at: AtomicU64::new(0),
                pending_len: AtomicU64::new(0),
                pending_generation: AtomicU64::new(0),
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
                generation: 0,
                base: 0,
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

    /// Every sample ever accepted into the ring.
    #[cfg(test)]
    pub(crate) fn accepted_total(&self) -> u64 {
        self.inner.accepted.load(Ordering::Acquire)
    }

    /// Audio thread: fold one block of output into the tap as mono.
    ///
    /// Allocation-free and lock-free, and must only ever be called from one
    /// thread. Returns immediately when disabled, so a running invention
    /// nobody is watching pays one relaxed load per block.
    ///
    /// Channels are summed and halved; a signal panned hard to one side
    /// therefore reads 6 dB below its channel peak, and an out-of-phase pair
    /// cancels.
    #[inline]
    pub(crate) fn observe_block(&self, left: &[f32], right: &[f32], frames: usize) {
        if !self.is_enabled() {
            return;
        }
        let inner = &self.inner;
        let frames = frames.min(left.len()).min(right.len());
        let generation = inner.generation.load(Ordering::Acquire);
        let read = inner.read.load(Ordering::Acquire);
        let mut write = inner.write.load(Ordering::Relaxed);
        let mut accepted = inner.accepted.load(Ordering::Relaxed);

        // A loss is published before any audio that follows it, so the reader
        // always learns of a hole before it can read past it. Until it can be
        // published, audio keeps being dropped into it.
        let blocked = inner.pending_len.load(Ordering::Relaxed) > 0
            && !inner.publish_pending(generation, write, read);

        let mut dropped = 0u64;
        for i in 0..frames {
            let next = if write + 1 == CAPACITY { 0 } else { write + 1 };
            if blocked || next == read {
                // Full, or a loss is still waiting to be published. Drop the
                // rest of the block rather than overwrite unread samples.
                dropped = (frames - i) as u64;
                break;
            }
            let mono = 0.5 * (left[i] + right[i]);
            inner.samples[write].store(mono.to_bits(), Ordering::Relaxed);
            write = next;
            accepted += 1;
        }
        inner.accepted.store(accepted, Ordering::Release);
        inner.write.store(write, Ordering::Release);
        if dropped > 0 {
            inner.extend_pending(accepted, dropped, generation);
        }
    }
}

impl TapInner {
    /// Writer only: publishes the loss being dropped, once there is room for
    /// what follows it. Returns whether nothing is left pending.
    fn publish_pending(&self, generation: u64, write: usize, read: usize) -> bool {
        if self.pending_generation.load(Ordering::Relaxed) != generation {
            // Left over from before a restart; that stream is gone.
            self.pending_len.store(0, Ordering::Relaxed);
            return true;
        }
        let next = if write + 1 == CAPACITY { 0 } else { write + 1 };
        if next == read {
            // Still full: the loss is still going on.
            return false;
        }
        let tail = self.hole_tail.load(Ordering::Relaxed);
        let next_tail = (tail + 1) % HOLE_CAPACITY;
        if next_tail == self.hole_head.load(Ordering::Acquire) {
            // Nowhere to publish it. Rather than lose track of where this loss
            // sits, keep dropping, so it stays one loss at one position.
            return false;
        }
        let record = &self.holes[tail];
        record
            .at
            .store(self.pending_at.load(Ordering::Relaxed), Ordering::Relaxed);
        record
            .len
            .store(self.pending_len.load(Ordering::Relaxed), Ordering::Relaxed);
        record.generation.store(generation, Ordering::Relaxed);
        // Publishes the record; the reader acquires this before reading it.
        self.hole_tail.store(next_tail, Ordering::Release);
        self.pending_len.store(0, Ordering::Relaxed);
        true
    }

    /// Writer only: adds `len` dropped samples to the loss in progress, or
    /// starts one after `at` accepted samples.
    fn extend_pending(&self, at: u64, len: u64, generation: u64) {
        self.dropped_total.fetch_add(len, Ordering::Relaxed);
        let pending = self.pending_len.load(Ordering::Relaxed);
        if pending > 0 && self.pending_generation.load(Ordering::Relaxed) == generation {
            // Nothing has been accepted since this loss began, so it is the
            // same loss at the same position.
            debug_assert_eq!(self.pending_at.load(Ordering::Relaxed), at);
            self.pending_len.store(pending + len, Ordering::Relaxed);
        } else {
            self.pending_at.store(at, Ordering::Relaxed);
            self.pending_len.store(len, Ordering::Relaxed);
            self.pending_generation.store(generation, Ordering::Relaxed);
        }
    }
}

/// The analyser's end of a [`SpectrumTap`]: the only consumer, and the only
/// thing that stores to the read index.
///
/// Positions it reports count from the start of the current stream, so a
/// second stream on the same tap sees its losses where they happened in *its*
/// audio. Dropping it stops collection and returns the reading end, so a
/// stream that ends by an early return or a panic cannot leave the audio
/// thread filling a ring nobody drains.
pub struct SpectrumReader {
    inner: Arc<TapInner>,
    /// The tap's generation when this stream started.
    generation: u64,
    /// `taken` when this stream started: its position zero.
    base: u64,
}

impl SpectrumReader {
    /// Starts a stream: discards anything buffered, so analysis begins from
    /// live audio rather than stale samples, and counts positions from here.
    ///
    /// A block the audio thread already had in flight may still land after
    /// this; it is taken as the stream's first audio.
    pub fn start(&mut self) {
        self.discard_buffered();
        self.generation = self.inner.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.base = self.inner.taken.load(Ordering::Relaxed);
        self.inner.enabled.store(true, Ordering::Release);
    }

    /// Stops collection.
    pub fn stop(&mut self) {
        self.inner.enabled.store(false, Ordering::Relaxed);
    }

    /// Whether collection is running.
    pub fn is_enabled(&self) -> bool {
        self.inner.enabled.load(Ordering::Relaxed)
    }

    /// Samples taken since this stream started, read or discarded: where the
    /// next sample to be read sits in the stream's arrived audio.
    pub fn position(&self) -> u64 {
        self.inner.taken.load(Ordering::Relaxed) - self.base
    }

    /// Samples waiting to be read.
    ///
    /// Any loss preceding these samples has already been published, so call
    /// this before [`take_hole`](Self::take_hole) and read no more than it
    /// reports to be sure of never reading past a loss unannounced.
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
        self.advance_taken(count as u64);
        count
    }

    /// The next loss in this stream, as its [`position`](Self::position) and
    /// the number of samples lost there. Losses come oldest first, and each
    /// is reported once.
    ///
    /// The caller applies a loss when its position reaches it, not when it
    /// first hears of it, which may be up to a ring of good audio too early.
    pub fn take_hole(&mut self) -> Option<(u64, u64)> {
        loop {
            let head = self.inner.hole_head.load(Ordering::Relaxed);
            if head == self.inner.hole_tail.load(Ordering::Acquire) {
                return None;
            }
            let record = &self.inner.holes[head];
            let at = record.at.load(Ordering::Relaxed);
            let len = record.len.load(Ordering::Relaxed);
            let generation = record.generation.load(Ordering::Relaxed);
            // Frees the record; the writer acquires this before reusing it.
            self.inner
                .hole_head
                .store((head + 1) % HOLE_CAPACITY, Ordering::Release);
            if generation != self.generation || at < self.base {
                // From an earlier stream on this tap.
                continue;
            }
            return Some((at - self.base, len));
        }
    }

    /// Every sample ever dropped because analysis fell behind.
    pub fn dropped_total(&self) -> u64 {
        self.inner.dropped_total.load(Ordering::Relaxed)
    }

    fn advance_taken(&self, count: u64) {
        let taken = self.inner.taken.load(Ordering::Relaxed);
        self.inner.taken.store(taken + count, Ordering::Relaxed);
    }

    /// Drops everything buffered, along with every published loss.
    fn discard_buffered(&mut self) {
        let write = self.inner.write.load(Ordering::Acquire);
        let read = self.inner.read.load(Ordering::Relaxed);
        let skipped = (write + CAPACITY - read) % CAPACITY;
        self.inner.read.store(write, Ordering::Release);
        self.advance_taken(skipped as u64);
        let tail = self.inner.hole_tail.load(Ordering::Acquire);
        self.inner.hole_head.store(tail, Ordering::Release);
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
mod tests;
