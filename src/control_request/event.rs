//! Counted events that a last-write-wins value would lose.

use super::sync::{AtomicU32, Ordering};

/// Counts events (a queue overflow, a re-seed, a trigger) so that two events
/// between two looks are two, not one overwritten value.
///
/// Any thread may [`record`](Self::record); a reader observes new events
/// through an [`EventCursor`].
///
/// Ordering: `record` is a `Release` `fetch_add` and the cursor's load is
/// `Acquire`. A cursor that sees an event therefore also sees whatever the
/// recording thread stored before it; with several recorders, each
/// `fetch_add` continues the others' release sequence, so the latest count
/// seen publishes every counted recorder's earlier stores.
pub(crate) struct EventCounter {
    count: AtomicU32,
}

impl EventCounter {
    pub(crate) fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
        }
    }

    /// A counter that has already counted `count` events, so tests can
    /// reach the wrap.
    #[cfg(test)]
    pub(crate) fn starting_at(count: u32) -> Self {
        Self {
            count: AtomicU32::new(count),
        }
    }

    /// Records one event. Lock-free and allocation-free from any thread.
    #[inline]
    pub(crate) fn record(&self) {
        self.count.fetch_add(1, Ordering::Release);
    }

    /// Events recorded so far, modulo 2^32.
    #[inline]
    pub(crate) fn count(&self) -> u32 {
        self.count.load(Ordering::Acquire)
    }
}

/// A reader's position in an [`EventCounter`].
///
/// A new cursor starts at zero, so its first [`take`](Self::take) reports
/// every event since the counter was created.
#[derive(Default)]
pub(crate) struct EventCursor {
    seen: u32,
}

impl EventCursor {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns how many events were recorded since the last call.
    /// Wait-free and allocation-free.
    ///
    /// The count is taken modulo 2^32 (`wrapping_sub`), so it is exact as
    /// long as fewer than 2^32 events arrive between two calls.
    #[inline]
    pub(crate) fn take(&mut self, counter: &EventCounter) -> u32 {
        let count = counter.count();
        let new = count.wrapping_sub(self.seen);
        self.seen = count;
        new
    }
}
