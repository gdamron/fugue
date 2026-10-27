//! Writes timed on a sample whose outcome the audio thread may have
//! dropped.
//!
//! Dropped outcomes name no request, so a write timed on a sample that was
//! waiting then may have settled, or may still wait. It stays listed until
//! its own outcome arrives or until the graph has surely settled it: then,
//! still here, its outcome was lost, and it is taken to have applied, as
//! most do.
//!
//! "Surely" takes two reads of the request queue:
//! - The drain has taken up every request queued when the drop was seen,
//!   the write among them. Until then it may still wait in the queue (for
//!   room in the store, or behind an edit that cannot install yet), however
//!   far rendering goes.
//! - The graph has then rendered past both its sample and the count read
//!   once it was seen taken up. Taken up in a block that started by that
//!   count, it waits in the store and settles in the block that holds its
//!   sample, or in that first block when its sample passed. So every
//!   outcome it could have was queued or dropped by a count read later,
//!   before receiving outcomes.
//!
//! Taken to have applied that late, it may have been overtaken: another
//! authored write to its control, applied on a later sample, may have been
//! recorded meanwhile. The document then keeps the later one, as playback
//! does, and the suspect is announced but not recorded.
//!
//! Exceptions remain:
//! - A write for a generation not yet installed (one a timed edit, waiting
//!   for its time, will install) is held in the store until that install.
//!   Taken to have applied meanwhile, its own outcome then finds no write
//!   and is counted lost.
//! - Overtaking is told by its own sample, so one that applied late, after
//!   a write recorded for a sample between, is not recorded. And it is told
//!   by key, so a whole value (`degrees`) does not overtake a part of it
//!   (`degree.1`).

use std::collections::{HashMap, VecDeque};

use super::{PendingLog, PendingWrite};
use crate::control_request::{Outcome, RequestId, When};

/// Each control's latest authored write that applied, by module and key:
/// its sample, then its request, which orders writes on one sample.
#[derive(Default)]
pub(super) struct Latest(HashMap<String, HashMap<String, (u64, RequestId)>>);

impl Latest {
    /// Notes authored `write`, request `id`, as `outcome` applied it.
    pub(super) fn applied(&mut self, write: &PendingWrite, id: RequestId, outcome: Outcome) {
        if let Outcome::Applied { at } | Outcome::AppliedLate { at, .. } = outcome {
            self.note(write, id, at);
        }
    }

    fn note(&mut self, write: &PendingWrite, id: RequestId, at: u64) {
        if write.authored.is_none() {
            return;
        }
        let keys = match self.0.get_mut(&write.module_id) {
            Some(keys) => keys,
            None => self.0.entry(write.module_id.clone()).or_default(),
        };
        match keys.get_mut(&write.key) {
            Some(latest) => *latest = (*latest).max((at, id)),
            None => {
                keys.insert(write.key.clone(), (at, id));
            }
        }
    }

    /// Whether an authored write to `write`'s control applied after it,
    /// request `id` on sample `at`.
    fn overtaken(&self, write: &PendingWrite, id: RequestId, at: u64) -> bool {
        (self.0.get(&write.module_id))
            .and_then(|keys| keys.get(&write.key))
            .is_some_and(|&latest| latest > (at, id))
    }
}

/// A write that may have lost its outcome.
pub(super) struct Suspect {
    id: RequestId,
    /// Its sample.
    at: u64,
    /// The queue's position when the drop was seen.
    queued: u64,
    /// Seen taken up by the drain.
    taken: bool,
    /// Past which count it has surely settled, once seen taken up.
    by: Option<u64>,
}

impl PendingLog {
    /// Notes which suspect writes the drain has surely taken up, then reads
    /// the rendered count: every check before the one read, so the count is
    /// at least where each was taken up (see the module docs).
    pub(super) fn taken_up(&mut self) -> u64 {
        let requests = &self.requests;
        for suspect in self
            .suspect
            .iter_mut()
            .filter(|suspect| suspect.by.is_none())
        {
            suspect.taken = requests.taken_before(suspect.queued);
        }
        let rendered = requests.rendered();
        for suspect in self.suspect.iter_mut().filter(|suspect| suspect.taken) {
            suspect.by.get_or_insert(suspect.at.max(rendered));
        }
        rendered
    }

    /// After `dropped` outcomes were lost. Which applied cannot be known;
    /// most do, and an author's write is not to be lost from the document,
    /// so an untimed one is taken to have applied. One timed on a sample
    /// may still wait, so it stays until its own outcome or until it has
    /// surely settled. One timed on a beat or a condition may have settled
    /// or may wait, so it stays too: listed until its outcome, or until it
    /// is forgotten (see `submitted`) or the invention stops.
    pub(super) fn lost_outcomes(&mut self, dropped: u32) {
        self.lost += u64::from(dropped);
        self.log.lost(u64::from(dropped));
        let writes = std::mem::take(&mut self.writes);
        let (settled, waiting): (VecDeque<_>, VecDeque<_>) =
            (writes.into_iter()).partition(|(_, write)| write.when == When::Now);
        self.writes = waiting;
        for (id, write) in settled {
            self.authored(&write);
            self.announce(id, &write, None);
        }
        self.suspect_lost();
        self.timed.clear();
        self.timed_lost = true;
    }

    /// Marks every write timed on a sample still listed as maybe having
    /// lost its outcome.
    fn suspect_lost(&mut self) {
        let queued = self.requests.queued();
        for (id, write) in &self.writes {
            let When::AtSample(at) = write.when else {
                continue;
            };
            if !self.suspect.iter().any(|suspect| suspect.id == *id) {
                let (id, taken, by) = (*id, false, None);
                self.suspect.push(Suspect {
                    id,
                    at,
                    queued,
                    taken,
                    by,
                });
            }
        }
    }

    /// Takes each suspect write the graph has surely settled to have
    /// applied, `rendered` being the count read before receiving outcomes.
    /// One settled by its own outcome meanwhile is no longer listed.
    pub(super) fn settle_lost(&mut self, rendered: u64) {
        if self.suspect.is_empty() {
            return;
        }
        let mut suspect = std::mem::take(&mut self.suspect);
        suspect.retain(|suspect| {
            let at = (self.writes.iter()).position(|(listed, _)| *listed == suspect.id);
            let Some(at) = at else {
                return false;
            };
            if suspect.by.is_none_or(|by| by >= rendered) {
                return true;
            }
            if let Some((id, write)) = self.writes.remove(at) {
                if !self.latest.overtaken(&write, id, suspect.at) {
                    self.latest.note(&write, id, suspect.at);
                    self.authored(&write);
                }
                self.announce(id, &write, None);
            }
            false
        });
        self.suspect = suspect;
    }
}
