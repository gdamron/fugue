//! The sample transport: the first layer of musical time.
//!
//! The engine counts samples since its graph was created: Fugue always runs
//! from load, with no transport start or stop, and the count never goes
//! back. The audio thread owns the count (`SignalGraph::current_sample`)
//! and publishes it here after every block; control threads read it to
//! time requests (see [`RequestSender::submit`](super::RequestSender::submit)).
//! There is no beat or tempo at this layer: clock timelines build on it.
//!
//! Offline render runs the same `process_block` over the same counter, so a
//! request timed in samples lands on the same sample there as live.

use super::sync::{AtomicU64, Ordering};

/// The engine's published sample count. Shared between the audio thread,
/// its only writer, and any number of control-thread readers.
///
/// One atomic word with one writer, so `Relaxed` is enough on both sides:
/// per-location coherence means no reader ever sees the count go back, and
/// nothing else is published through it (a request carries its own data
/// through the queue, which orders it).
pub(crate) struct Transport {
    /// Samples the audio thread has rendered, which is also the sample the
    /// next block starts at.
    rendered: AtomicU64,
}

impl Transport {
    pub(crate) fn new() -> Self {
        Self {
            rendered: AtomicU64::new(0),
        }
    }

    /// Publishes the count after a block. Audio thread only: wait-free,
    /// allocation- and lock-free.
    #[inline]
    pub(crate) fn publish(&self, rendered: u64) {
        self.rendered.store(rendered, Ordering::Relaxed);
    }

    /// Samples rendered as of the latest block the audio thread finished:
    /// the earliest sample a request submitted now can apply at. The audio
    /// thread may be part way into the next block, so a request timed close
    /// to this can still arrive after its sample and apply late.
    #[inline]
    pub(crate) fn rendered(&self) -> u64 {
        self.rendered.load(Ordering::Relaxed)
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self::new()
    }
}
