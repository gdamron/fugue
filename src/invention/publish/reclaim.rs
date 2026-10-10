//! Freeing what the audio thread retires, on a control thread.
//!
//! Every publication hands the previous module map back on the retire
//! channel, holding removed and replaced instances. Dropping a removed sink
//! finalizes its file or stream, so retired publications are freed on a
//! short cadence rather than whenever the next change happens to start.
//! The same cadence frees the engine's retired request payloads (see
//! `crate::payload`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::publisher::RetireRing;
use crate::payload::RetireQueue;

/// How often the reclaimer thread frees retired publications.
pub(crate) const RECLAIM_INTERVAL: Duration = Duration::from_millis(25);

/// The control side of a live graph's retire ring. Its pops take the
/// ring's consumer lock, which the audio thread's pushes never touch.
pub(crate) struct Reclaimer {
    retired: Arc<RetireRing>,
    /// The engine's retired request payloads.
    payloads: Arc<RetireQueue>,
    /// Publications freed so far.
    freed: AtomicU64,
    /// Whether a reclaimer thread runs (see [`Self::spawn`]).
    threaded: AtomicBool,
}

impl Reclaimer {
    /// Frees what arrives on `retired` and `payloads` (the queue the
    /// engine's request drain retires payloads to).
    pub(crate) fn new(retired: Arc<RetireRing>, payloads: Arc<RetireQueue>) -> Self {
        Self {
            retired,
            payloads,
            freed: AtomicU64::new(0),
            threaded: AtomicBool::new(false),
        }
    }

    /// Publications freed so far. Every publication is freed once the audio
    /// side has installed it and a later one has replaced it.
    pub(crate) fn freed(&self) -> u64 {
        self.freed.load(Ordering::Relaxed)
    }

    /// Whether a reclaimer thread frees retirements on its own cadence;
    /// without one, only control-thread changes free them.
    pub(crate) fn threaded(&self) -> bool {
        self.threaded.load(Ordering::Relaxed)
    }

    /// Frees every publication retired so far and returns how many. Each
    /// is taken under the ring's consumer lock but dropped after releasing
    /// it, because a sink may block while it finalizes. Never takes the
    /// publisher's lock. Also frees the retired payloads, uncounted.
    pub(crate) fn reclaim(&self) -> usize {
        let freed = self.retired.drain();
        self.freed.fetch_add(freed as u64, Ordering::Relaxed);
        self.payloads.drain();
        freed
    }

    /// Starts a thread that reclaims every [`RECLAIM_INTERVAL`] until
    /// `reclaimer` is dropped. Returns false when no thread could start; the
    /// publisher's own changes still reclaim before they prepare.
    pub(crate) fn spawn(reclaimer: &Arc<Self>) -> bool {
        let threaded = &reclaimer.threaded;
        let reclaimer = Arc::downgrade(reclaimer);
        let started = std::thread::Builder::new()
            .name("fugue-reclaim".to_string())
            .spawn(move || loop {
                std::thread::sleep(RECLAIM_INTERVAL);
                let Some(reclaimer) = reclaimer.upgrade() else {
                    break;
                };
                reclaimer.reclaim();
            })
            .is_ok();
        threaded.store(started, Ordering::Relaxed);
        started
    }
}
