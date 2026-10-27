//! Writes timed on a sample whose outcome the audio thread may have
//! dropped.
//!
//! Dropped outcomes name no request, so a write timed on a sample that was
//! waiting then may have settled, or may still wait for its sample. It
//! stays listed until its own outcome arrives or until the graph has surely
//! rendered past it: then, still here, its outcome was lost, and it is
//! taken to have applied, as most do.
//!
//! "Surely" counts from the rendered count read before the drop was seen
//! (`seen`). The write was queued before that, while the block from `seen`
//! at the latest was rendering, so the audio thread took it up by the next
//! block, at most [`MAX_BLOCK`] on. From there it waits in the store and
//! settles in the block that holds its sample, or in the first block when
//! that sample passed. Once a count read before receiving outcomes is past
//! both, every outcome of it was queued or dropped by then. A write left in
//! the queue for want of room in the store is the exception: taken up
//! later, it may settle after it was taken to have applied. Its outcome then
//! finds no write and is counted lost.

use super::PendingLog;
use crate::control_request::When;
use crate::MAX_BLOCK;

impl PendingLog {
    /// Marks every write timed on a sample still listed as maybe having
    /// lost its outcome, `seen` being the count read before the drop was
    /// seen.
    pub(super) fn suspect_lost(&mut self, seen: u64) {
        let taken_up = seen.saturating_add(MAX_BLOCK as u64);
        for (id, write) in &self.writes {
            let When::AtSample(at) = write.when else {
                continue;
            };
            if !self.suspect.iter().any(|(suspect, _)| suspect == id) {
                self.suspect.push((*id, at.max(taken_up)));
            }
        }
    }

    /// Takes each suspect write the graph has surely rendered past to have
    /// applied, `rendered` being the count read before receiving outcomes.
    /// One settled by its own outcome meanwhile is no longer listed.
    pub(super) fn settle_lost(&mut self, rendered: u64) {
        if self.suspect.is_empty() {
            return;
        }
        let mut suspect = std::mem::take(&mut self.suspect);
        suspect.retain(|&(id, by)| {
            let at = self.writes.iter().position(|(listed, _)| *listed == id);
            let Some(at) = at else {
                return false;
            };
            if by >= rendered {
                return true;
            }
            if let Some((id, write)) = self.writes.remove(at) {
                self.authored(&write);
                self.announce(id, &write, None);
            }
            false
        });
        self.suspect = suspect;
    }
}
