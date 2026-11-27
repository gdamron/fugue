//! The notes each module has played, kept for readers that follow them (a
//! piano roll over a sequencer, an agent asking what was played) and read
//! by cursor: the daemon's `PollNotes` and the browser engine's played
//! notes both keep theirs here, so both answer alike.
//!
//! Notes have a history of their own rather than a place in an event log:
//! a sequencer plays several a second, which would evict the control
//! changes a log exists to keep. Sequence numbers start where the owner
//! says (the daemon starts them at its start time in microseconds, so a
//! cursor from an earlier daemon is never mistaken for a current one), the
//! history is bounded, and a cursor it cannot continue is told so. Each
//! note is kept once, as it now stands, so a page carries a note's start
//! and end as one entry. A module's history is on one clock: beats on
//! another clock are another timeline, so a module that moves to another
//! clock starts over.

use std::collections::{BTreeMap, HashMap};

use super::{NoteChange, NoteChanges, Playhead};
use crate::rpc::{NotePage, PlayedNote};

/// The notes kept for each module. At a note a beat this is over four
/// minutes of a sequencer at 120 BPM, past the bars a piano roll shows; a
/// reader polling every second or so never falls behind it.
pub const NOTES_PER_MODULE: usize = 512;

/// The notes each module has played, by cursor.
pub struct NoteHistory {
    /// The sequence number of the next change.
    next_seq: u64,
    /// No cursor at or below this can be continued, for any module: the
    /// changes after it belong to an earlier run, or some were missed.
    lost_through: u64,
    /// The run these notes were played in.
    run: u64,
    modules: HashMap<String, ModuleNotes>,
}

#[derive(Default)]
struct ModuleNotes {
    /// By note number: the order notes started in.
    notes: BTreeMap<u64, Kept>,
    /// The newest change pruned from this module's history, or the mark
    /// left as it started over on another clock.
    pruned_through: u64,
    /// The clock its notes' beats count on; `None` without one.
    clock: Option<String>,
}

struct Kept {
    seq: u64,
    change: NoteChange,
}

impl NoteHistory {
    /// An empty history whose first change is numbered `first_seq` (at
    /// least 1, since 0 asks for everything kept). Every cursor below it
    /// is told to resync.
    pub fn starting_at(first_seq: u64) -> Self {
        let first = first_seq.max(1);
        Self {
            next_seq: first,
            lost_through: first - 1,
            run: 0,
            modules: HashMap::new(),
        }
    }

    /// The sequence number of the latest change, or one below the first
    /// before any.
    pub fn latest_seq(&self) -> u64 {
        self.next_seq - 1
    }

    /// Records what run `run` played since the last drain. A new run starts
    /// the history over, and missed changes make every cursor resync.
    pub fn record(&mut self, run: u64, changes: NoteChanges) {
        if run != self.run {
            self.modules.clear();
            self.lose_everything_so_far();
            self.run = run;
        }
        for change in changes.notes {
            let module = self.modules.entry(change.module_id.clone()).or_default();
            if module.clock != change.clock {
                if !module.notes.is_empty() {
                    module.notes.clear();
                    module.pruned_through = self.next_seq;
                    self.next_seq += 1;
                }
                module.clock = change.clock.clone();
            }
            let seq = self.next_seq;
            self.next_seq += 1;
            module.notes.insert(change.note, Kept { seq, change });
            module.prune();
        }
        if changes.missed > 0 {
            // The notes held may have ended among the missed changes, and
            // the feed has forgotten them: no end will come.
            for module in self.modules.values_mut() {
                module.notes.retain(|_, kept| kept.change.end.is_some());
            }
            self.lose_everything_so_far();
        }
    }

    /// Makes every cursor issued so far resync: a sequence number of its
    /// own marks the break, so even the latest cursor is behind it.
    fn lose_everything_so_far(&mut self) {
        self.lost_through = self.next_seq;
        self.next_seq += 1;
    }

    /// The notes of `module_id` changed after `after` (`None` or `0`: every
    /// one kept), their seconds counted at `sample_rate`. The page is on
    /// the clock of the module's notes or, with none kept, of `playhead`,
    /// if any; the playhead is left out while it is on another clock than
    /// the notes.
    pub fn page(
        &self,
        module_id: &str,
        after: Option<u64>,
        playhead: Option<Playhead>,
        sample_rate: u32,
    ) -> NotePage {
        let latest_seq = self.latest_seq();
        let module = self.modules.get(module_id);
        let lost = module.map_or(0, |module| module.pruned_through);
        let lost = lost.max(self.lost_through);
        let after = after.filter(|&after| after != 0);
        let resync_required = after.is_some_and(|after| after < lost || after > latest_seq);
        let since = after.filter(|_| !resync_required).unwrap_or(0);
        let clock = match module {
            Some(module) if !module.notes.is_empty() => module.clock.clone(),
            _ => playhead.as_ref().map(|playhead| playhead.clock.clone()),
        };
        let playhead = playhead.filter(|playhead| clock.as_ref() == Some(&playhead.clock));
        let mut notes: Vec<PlayedNote> = module
            .into_iter()
            .flat_map(|module| module.notes.values())
            .filter(|kept| kept.seq > since)
            .map(|Kept { seq, change }| PlayedNote {
                seq: *seq,
                id: change.note.to_string(),
                note: change.listed(sample_rate),
            })
            .collect();
        notes.sort_unstable_by_key(|note| note.seq);
        NotePage {
            module_id: module_id.to_string(),
            clock,
            notes,
            latest_seq,
            resync_required,
            playhead: playhead.map(|playhead| playhead.beats),
        }
    }
}

impl ModuleNotes {
    /// Drops the oldest finished notes past the cap, or the oldest notes if
    /// every one is held.
    fn prune(&mut self) {
        while self.notes.len() > NOTES_PER_MODULE {
            let oldest = self
                .notes
                .iter()
                .find(|(_, kept)| kept.change.end.is_some())
                .or_else(|| self.notes.iter().next())
                .map(|(&note, _)| note);
            let Some(kept) = oldest.and_then(|note| self.notes.remove(&note)) else {
                return;
            };
            self.pruned_through = self.pruned_through.max(kept.seq);
        }
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
