//! Position writes in flight, and the edits that wait them out (see the
//! module docs on generations coming back round).

use std::sync::atomic::{AtomicU64, Ordering};

use super::parts::drains;
use super::DegreeTable;

impl DegreeTable {
    /// Marks a position write in flight, in the current epoch, until the
    /// guard drops. Taken before the write reads the generation it checks
    /// against, so an edit about to publish that generation again waits it
    /// out. Never waits: a retry means an edit moved the epoch meanwhile.
    pub(super) fn in_flight(&self) -> InFlight<'_> {
        loop {
            let epoch = self.epoch.load(Ordering::SeqCst);
            let count = &self.writes[(epoch & 1) as usize];
            count.fetch_add(1, Ordering::SeqCst);
            if self.epoch.load(Ordering::SeqCst) == epoch {
                return InFlight(count);
            }
            // Marked after the epoch moved on: an edit may be waiting on
            // this count, and need not wait for this write.
            count.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Waits until every position write marked in flight so far has
    /// finished, should publishing `generation` call for it: moves the
    /// epoch on, so writes from now on mark the other count, and waits for
    /// the one before to empty. Control thread, under the edit lock; writes
    /// never wait for an edit, so this ends.
    pub(super) fn drain_before(&self, generation: u32) {
        if !drains(generation) {
            return;
        }
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst);
        let count = &self.writes[(epoch & 1) as usize];
        #[cfg(test)]
        let waiting = std::time::Instant::now();
        while count.load(Ordering::SeqCst) != 0 {
            #[cfg(test)]
            super::tests::still_waiting(waiting, "the writes in flight");
            std::thread::yield_now();
        }
    }
}

/// A position write in flight (see [`DegreeTable::in_flight`]): the count
/// it is marked in.
pub(super) struct InFlight<'a>(&'a AtomicU64);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
