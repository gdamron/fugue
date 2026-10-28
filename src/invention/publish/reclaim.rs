//! Freeing what the audio thread retires, on a control thread.
//!
//! Every publication hands the previous module map back on the retire
//! channel, holding removed and replaced instances. Dropping a removed sink
//! finalizes its file or stream, so retired publications are freed on a
//! short cadence rather than whenever the next change happens to start.
//! The same cadence frees the engine's retired request payloads (see
//! `crate::payload`). With no reclaimer thread (wasm32 has none), changes
//! and control writes reclaim on their own control thread instead. A host
//! with threads treats a failed spawn as a failed start (see
//! `InventionRuntime::start_with_backend`).

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
    /// without one, control-thread changes and writes free them.
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

    /// [`Self::reclaim`] on the calling control thread, unless a reclaimer
    /// thread frees retirements on its own cadence. Without one, every
    /// change and every control write frees what the audio side retired:
    /// so payload requests waiting for retire room, far-future ones
    /// reserving it included, and the edits queued behind them, wait for
    /// the next write or change, never for a later change alone.
    pub(crate) fn reclaim_unthreaded(&self) -> usize {
        match self.threaded() {
            true => 0,
            false => self.reclaim(),
        }
    }

    /// Starts a thread that reclaims, then runs `tick`, every
    /// [`RECLAIM_INTERVAL`] until `reclaimer` is dropped.
    ///
    /// # Errors
    ///
    /// The operating system's refusal when no thread could start. Changes
    /// and control writes still reclaim before they lock the publisher, but
    /// nothing queues the note offs the request channel holds for want of
    /// room until the next write, so a host that can will not run without
    /// the thread.
    pub(crate) fn spawn(
        reclaimer: &Arc<Self>,
        tick: impl Fn() + Send + 'static,
    ) -> std::io::Result<()> {
        Self::spawn_with(reclaimer, tick, |name, body| {
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(body)
                .map(drop)
        })
    }

    /// [`Self::spawn`], starting the thread through `start`, so a test can
    /// refuse it.
    fn spawn_with(
        reclaimer: &Arc<Self>,
        tick: impl Fn() + Send + 'static,
        start: impl FnOnce(&str, Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        let weak = Arc::downgrade(reclaimer);
        let started = start(
            "fugue-reclaim",
            Box::new(move || loop {
                std::thread::sleep(RECLAIM_INTERVAL);
                let Some(reclaimer) = weak.upgrade() else {
                    break;
                };
                reclaimer.reclaim();
                tick();
            }),
        );
        reclaimer.threaded.store(started.is_ok(), Ordering::Relaxed);
        started
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::invention::graph::Publication;
    use crate::spsc::Ring;

    fn reclaimer() -> Arc<Reclaimer> {
        Arc::new(Reclaimer::new(
            Ring::<Box<Publication>>::with_capacity(1),
            RetireQueue::with_capacity(4),
        ))
    }

    #[test]
    fn a_refused_thread_is_an_error_and_leaves_the_reclaimer_unthreaded() {
        let reclaimer = reclaimer();
        let refused = Reclaimer::spawn_with(
            &reclaimer,
            || (),
            |_, _| Err(std::io::Error::other("no threads left")),
        );

        let error = refused.expect_err("the thread was refused");
        assert_eq!(error.to_string(), "no threads left");
        assert!(!reclaimer.threaded());
    }

    #[test]
    fn a_started_thread_marks_the_reclaimer_threaded() {
        let reclaimer = reclaimer();
        Reclaimer::spawn(&reclaimer, || ()).expect("a thread starts");
        assert!(reclaimer.threaded());
    }
}
