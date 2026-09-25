use std::sync::atomic::{fence, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

/// Samples the ring holds: about 0.7 s at 48 kHz, far more than the few
/// milliseconds between analysis passes. A power of two, so a position maps to
/// its slot with a mask.
pub(crate) const CAPACITY: usize = 32_768;
const MASK: usize = CAPACITY - 1;

/// The signal a master tap analyses, as spectrogram provenance names it.
const MASTER_SOURCE: &str = "sink:master";

/// The audio thread's end of a mono tap of the master output.
///
/// The write path allocates nothing, takes no locks, does no analysis and
/// never looks at a reader: it sums the two channels into a ring of samples
/// and publishes how many it has written. When the ring is full it overwrites
/// the oldest audio, so a reader that stalls resumes on fresh audio and is
/// told exactly how much it missed. Samples are held as raw `f32` bits in
/// atomics, so the handover needs no mutex and no `unsafe`.
///
/// Collection runs only while at least one [`SpectrumReader`] exists; with
/// none, the write path returns after a single relaxed load.
#[derive(Clone)]
pub(crate) struct SpectrumTap {
    inner: Arc<TapInner>,
}

struct TapInner {
    /// Ring storage, one `f32`'s bits per slot; sample `n` lives in slot
    /// `n % CAPACITY`.
    samples: Box<[AtomicU32]>,
    /// Samples ever written. Stored with `Release` after the samples it
    /// covers, so a reader that sees it also sees them.
    written: AtomicU64,
    /// Samples the writer has started writing. Raised before a block's samples
    /// are stored, so a reader can tell which of the samples it just copied
    /// may have been overwritten while it copied them.
    claimed: AtomicU64,
    /// Readers alive. The writer collects only while this is nonzero.
    readers: AtomicUsize,
}

impl SpectrumTap {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(TapInner {
                samples: (0..CAPACITY).map(|_| AtomicU32::new(0)).collect(),
                written: AtomicU64::new(0),
                claimed: AtomicU64::new(0),
                readers: AtomicUsize::new(0),
            }),
        }
    }

    /// Starts a reader at the live edge: it sees only audio written from now
    /// on, and collection runs for as long as it lives.
    pub(crate) fn reader(&self) -> SpectrumReader {
        self.inner.readers.fetch_add(1, Ordering::AcqRel);
        let next = self.inner.written.load(Ordering::Acquire);
        SpectrumReader {
            inner: Arc::clone(&self.inner),
            next,
            origin: next,
            lost_total: 0,
        }
    }

    /// Whether any reader is alive, and so whether the audio thread collects.
    #[inline]
    pub(crate) fn is_collecting(&self) -> bool {
        self.inner.readers.load(Ordering::Relaxed) > 0
    }

    /// Audio thread: fold one block of output into the tap as mono.
    ///
    /// Allocation-free and lock-free, and must only ever be called from one
    /// thread. Returns immediately when nobody is reading.
    ///
    /// Channels are summed and halved; a signal panned hard to one side
    /// therefore reads 6 dB below its channel peak, and an out-of-phase pair
    /// cancels.
    #[inline]
    pub(crate) fn observe_block(&self, left: &[f32], right: &[f32], frames: usize) {
        if !self.is_collecting() {
            return;
        }
        let inner = &*self.inner;
        let frames = frames.min(left.len()).min(right.len());
        let start = inner.written.load(Ordering::Relaxed);
        let end = start + frames as u64;

        // Claim the slots before overwriting them. The fence orders the claim
        // before every store below, so a reader that copies an overwritten
        // sample is guaranteed to see the claim that condemns it.
        inner.claimed.store(end, Ordering::Relaxed);
        fence(Ordering::Release);

        // At most two contiguous runs, split where the ring wraps, so the
        // loop body carries no per-sample wrap check.
        let mut slot = (start as usize) & MASK;
        let mut done = 0;
        while done < frames {
            let run = (frames - done).min(CAPACITY - slot);
            let slots = &inner.samples[slot..slot + run];
            let l = &left[done..done + run];
            let r = &right[done..done + run];
            for ((dst, a), b) in slots.iter().zip(l).zip(r) {
                dst.store((0.5 * (a + b)).to_bits(), Ordering::Relaxed);
            }
            done += run;
            slot = 0;
        }

        inner.written.store(end, Ordering::Release);
    }
}

/// The result of one [`SpectrumReader::read`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapRead {
    /// Samples lost immediately before the ones read: overwritten because the
    /// reader fell more than the ring behind.
    pub lost: u64,
    /// Samples copied out, contiguous and following the loss.
    pub count: usize,
}

/// An analyser's end of the master tap.
///
/// Any number may exist at once; none of them stores to anything the audio
/// thread reads, except to register on creation and deregister on drop.
/// Dropping the last one stops collection, so a stream that ends by an early
/// return or a panic cannot leave the audio thread filling a ring nobody
/// drains.
pub struct SpectrumReader {
    inner: Arc<TapInner>,
    /// Index, among all samples ever written, of the next one to read.
    next: u64,
    /// `next` when this reader started: its stream's position zero.
    origin: u64,
    lost_total: u64,
}

impl SpectrumReader {
    /// Samples passed since this reader started, read or lost: where the next
    /// sample sits in the stream's audio.
    pub fn position(&self) -> u64 {
        self.next - self.origin
    }

    /// Every sample this reader has lost because it fell behind.
    pub fn lost_total(&self) -> u64 {
        self.lost_total
    }

    /// Which signal this reader taps, as spectrogram provenance names it.
    pub fn source(&self) -> &'static str {
        MASTER_SOURCE
    }

    /// Copies up to `out.len()` of the oldest unread samples out, reporting
    /// how many samples were lost just before them.
    ///
    /// Losses only ever precede a read, never fall inside one, so the samples
    /// returned are always contiguous in the stream.
    pub fn read(&mut self, out: &mut [f32]) -> TapRead {
        let inner = &*self.inner;
        let written = inner.written.load(Ordering::Acquire);
        let oldest = written.saturating_sub(CAPACITY as u64);
        let start = self.next.max(oldest);
        let count = ((written - start) as usize).min(out.len());

        let mut slot = (start as usize) & MASK;
        for value in &mut out[..count] {
            *value = f32::from_bits(inner.samples[slot].load(Ordering::Relaxed));
            slot = (slot + 1) & MASK;
        }

        // Pairs with the writer's fence: if any copy above saw an overwrite,
        // this load sees the claim that covers it. Samples older than the
        // claim's reach may have been replaced mid-copy, so they count as lost.
        fence(Ordering::Acquire);
        let valid_from = inner
            .claimed
            .load(Ordering::Relaxed)
            .saturating_sub(CAPACITY as u64);
        let torn = (valid_from.saturating_sub(start) as usize).min(count);
        if torn > 0 {
            out.copy_within(torn..count, 0);
        }

        let lost = (start - self.next) + torn as u64;
        self.next = start + count as u64;
        self.lost_total += lost;
        TapRead {
            lost,
            count: count - torn,
        }
    }
}

impl Drop for SpectrumReader {
    fn drop(&mut self) {
        self.inner.readers.fetch_sub(1, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
