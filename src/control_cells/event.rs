//! Edge-like requests that last-writer-wins would lose.

use super::sync::{AtomicU32, Ordering};

/// Counts requests (re-seed, trigger, reset) so that two requests between
/// audio blocks are two requests, not one overwritten value.
///
/// Writer role: a parameter; any thread may call [`request`](Self::request).
/// The audio side observes requests through an [`EventCursor`].
///
/// Ordering: `request` is a `Release` `fetch_add` and the cursor's load is
/// `Acquire`. A cursor that sees a request therefore also sees whatever the
/// requesting thread stored before it (the value to re-seed with, say); with
/// several requesters, each `fetch_add` continues the others' release
/// sequence, so the latest count seen publishes every counted requester's
/// earlier stores.
pub(crate) struct EventCounter {
    count: AtomicU32,
}

impl EventCounter {
    pub(crate) fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
        }
    }

    /// A counter that has already counted `count` requests, so tests can
    /// reach the wrap.
    #[cfg(test)]
    pub(crate) fn starting_at(count: u32) -> Self {
        Self {
            count: AtomicU32::new(count),
        }
    }

    /// Records one request. Lock-free and allocation-free from any thread.
    #[inline]
    pub(crate) fn request(&self) {
        self.count.fetch_add(1, Ordering::Release);
    }

    /// Requests made so far, modulo 2^32 (`0` means "never requested" until
    /// the first wrap).
    #[inline]
    pub(crate) fn count(&self) -> u32 {
        self.count.load(Ordering::Acquire)
    }
}

/// The audio side's position in an [`EventCounter`].
///
/// A new cursor starts at zero, so its first [`take`](Self::take) reports
/// every request made since the counter was created.
#[derive(Default)]
pub(crate) struct EventCursor {
    seen: u32,
}

impl EventCursor {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns how many requests arrived since the last call. Lock-free,
    /// allocation-free and wait-free.
    ///
    /// The count is taken modulo 2^32 (`wrapping_sub`), so it is exact as
    /// long as fewer than 2^32 requests arrive between two calls.
    #[inline]
    pub(crate) fn take(&mut self, counter: &EventCounter) -> u32 {
        let count = counter.count();
        let new = count.wrapping_sub(self.seen);
        self.seen = count;
        new
    }
}
