//! Reserved requests (note offs) that found the queue full, held on the
//! control side until it has room.
//!
//! A note off refused for want of room leaves its note sounding, so a
//! reserved request that finds even the reserved slots taken (a burst of
//! more than `RESERVED_ROOM` behind stalled edits) waits here instead, and
//! its submission succeeds. What is held goes to the queue in the order it
//! came, before anything submitted after it: while any is held, a later
//! request is held behind it too if it is reserved, and refused
//! (`QueueFull`) as a full queue refuses it if not, edits included. So
//! nothing overtakes a held request, and nothing that could have reached
//! the queue is held up: one is held only while the queue has no slot at
//! all.
//!
//! The audio thread never sees this: it only frees slots, which the next
//! submission, or the reclaimer's tick, fills from here.

use std::collections::VecDeque;
use std::sync::MutexGuard;

use super::request::Request;
use super::sync::{Arc, AtomicBool, Ordering};
use crate::audio_thread::ControlLock;

/// Reserved requests a channel holds at most while its queue is full: as
/// many again as the queue's reserved room, for every voice of 64 more
/// sixteen-voice instruments. Past it, one is refused `QueueFull`.
pub(crate) const BACKLOG_CAPACITY: usize = 1024;

/// The held requests, oldest first.
#[derive(Default)]
pub(super) struct Backlog {
    held: ControlLock<VecDeque<Request>>,
    /// Whether any is held, read without the lock: a submission with none
    /// held checks nothing more.
    waiting: Arc<AtomicBool>,
}

impl Backlog {
    /// Whether any request is held.
    pub(super) fn waiting(&self) -> bool {
        self.waiting.load(Ordering::Acquire)
    }

    /// The flag [`Self::waiting`] reads, for a tick that flushes.
    pub(super) fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.waiting)
    }

    /// The held requests. Control threads only.
    pub(super) fn lock(&self) -> MutexGuard<'_, VecDeque<Request>> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Notes what `held` (this backlog's, locked) holds now.
    pub(super) fn update(&self, held: &VecDeque<Request>) {
        self.waiting.store(!held.is_empty(), Ordering::Release);
    }
}

/// Holds `request` behind those `held`, or hands it back when it is not
/// reserved or there is no room. Allocates the room once, on first use.
// Handed back whole, as the queue hands it back, so it is never dropped.
#[allow(clippy::result_large_err)]
pub(super) fn hold(held: &mut VecDeque<Request>, request: Request) -> Result<(), Request> {
    if !request.reserved || held.len() >= BACKLOG_CAPACITY {
        return Err(request);
    }
    if held.capacity() == 0 {
        held.reserve_exact(BACKLOG_CAPACITY);
    }
    held.push_back(request);
    Ok(())
}
