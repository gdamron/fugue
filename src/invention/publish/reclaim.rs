//! Freeing what the audio thread retires, on a control thread.
//!
//! Every publication hands the previous module map back on the retire
//! channel, holding removed and replaced instances. Dropping a removed sink
//! finalizes its file or stream, so retired publications are freed on a
//! short cadence rather than whenever the next change happens to start.
//! The same cadence frees the engine's retired request payloads (see
//! `crate::payload`).

use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::invention::graph::Publication;
use crate::payload::{RetireQueue, RETIRE_CAPACITY};

/// How often the reclaimer thread frees retired publications.
pub(crate) const RECLAIM_INTERVAL: Duration = Duration::from_millis(25);

/// The control side of a live graph's retire channel.
///
/// It only ever calls `try_recv`: a receiver parked in a blocking `recv`
/// would make the audio thread's `try_send` take the channel's waker lock.
pub(crate) struct Reclaimer {
    retired: Mutex<Receiver<Box<Publication>>>,
    /// The engine's retired request payloads.
    payloads: Arc<RetireQueue>,
}

impl Reclaimer {
    pub(crate) fn new(retired: Receiver<Box<Publication>>) -> Self {
        Self {
            retired: Mutex::new(retired),
            payloads: RetireQueue::with_capacity(RETIRE_CAPACITY),
        }
    }

    /// The queue the engine's request drain retires payloads to.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn payload_queue(&self) -> &Arc<RetireQueue> {
        &self.payloads
    }

    /// Frees every publication retired so far and returns how many. They
    /// are taken under this reclaimer's lock but dropped after releasing it,
    /// because a sink may block while it finalizes. Never takes the
    /// publisher's lock. Also frees the retired payloads, uncounted.
    pub(crate) fn reclaim(&self) -> usize {
        let retired: Vec<Box<Publication>> = {
            let receiver = self.retired.lock().unwrap();
            std::iter::from_fn(|| receiver.try_recv().ok()).collect()
        };
        self.payloads.drain();
        retired.len()
    }

    /// Starts a thread that reclaims every [`RECLAIM_INTERVAL`] until
    /// `reclaimer` is dropped. Returns false when no thread could start; the
    /// publisher's own changes still reclaim before they prepare.
    pub(crate) fn spawn(reclaimer: &Arc<Self>) -> bool {
        let reclaimer = Arc::downgrade(reclaimer);
        std::thread::Builder::new()
            .name("fugue-reclaim".to_string())
            .spawn(move || loop {
                std::thread::sleep(RECLAIM_INTERVAL);
                let Some(reclaimer) = reclaimer.upgrade() else {
                    break;
                };
                reclaimer.reclaim();
            })
            .is_ok()
    }
}
