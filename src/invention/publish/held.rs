//! Note offs the request channel holds for want of room, queued on the
//! reclaimer's cadence (see `control_request::backlog`), so they reach the
//! audio thread as it frees room whether or not anything else is
//! submitted.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{LiveGraph, Publisher};

/// Queues what the channel holds on each tick. Holds the live graph weakly,
/// so the thread never keeps it alive.
pub(crate) struct HeldFlush {
    /// Whether the channel holds any: checked first, so a tick costs one
    /// atomic load while it holds none.
    holding: Arc<AtomicBool>,
    publisher: Weak<Mutex<Publisher>>,
}

impl HeldFlush {
    pub(crate) fn tick(&self) {
        if !self.holding.load(Ordering::Acquire) {
            return;
        }
        let Some(publisher) = self.publisher.upgrade() else {
            return;
        };
        // Never waits: whoever holds the publisher submits, which queues
        // what is held first, and the next tick will if not.
        if let Ok(locked) = publisher.try_lock() {
            locked.flush_held();
        };
    }
}

impl LiveGraph {
    /// What queues the channel's held requests on the reclaimer's cadence.
    pub(crate) fn held_flush(&self) -> HeldFlush {
        HeldFlush {
            holding: self.requests.holding(),
            publisher: Arc::downgrade(&self.publisher),
        }
    }
}
