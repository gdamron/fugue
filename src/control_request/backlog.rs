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
use super::sync::{Arc, AtomicU64, Ordering};
use crate::audio_thread::ControlLock;

/// Reserved requests a channel holds at most while its queue is full: as
/// many again as the queue's reserved room, for every voice of 64 more
/// sixteen-voice instruments. Past it, one is refused `QueueFull`.
pub(crate) const BACKLOG_CAPACITY: usize = 1024;

/// The oldest graph generation a held request is tagged for, or none,
/// read without the lock. Requests are tagged as they are submitted, under
/// the publisher, so the oldest held is the first.
#[derive(Debug)]
pub(crate) struct Held(AtomicU64);

/// No request is held.
const NONE: u64 = u64::MAX;

impl Default for Held {
    fn default() -> Self {
        Self(AtomicU64::new(NONE))
    }
}

impl Held {
    /// Whether any request is held.
    pub(crate) fn any(&self) -> bool {
        self.0.load(Ordering::Acquire) != NONE
    }

    /// Whether a request tagged `generation` or older is held.
    pub(crate) fn through(&self, generation: u64) -> bool {
        self.0.load(Ordering::Acquire) <= generation
    }
}

/// The held requests, oldest first.
#[derive(Default)]
pub(super) struct Backlog {
    held: ControlLock<VecDeque<Request>>,
    /// What is held, read without the lock: a submission with none held
    /// checks nothing more.
    waiting: Arc<Held>,
}

impl Backlog {
    /// Whether any request is held.
    pub(super) fn waiting(&self) -> bool {
        self.waiting.any()
    }

    /// What [`Self::waiting`] reads, for a tick that flushes and for the
    /// drain.
    pub(super) fn flag(&self) -> Arc<Held> {
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
        let oldest = held
            .front()
            .map_or(NONE, |request| request.target.generation);
        self.waiting.0.store(oldest, Ordering::Release);
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
