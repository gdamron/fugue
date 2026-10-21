//! What became of each timed edit (see `timed`), for whoever submitted it:
//! queued, landed, adopted, or refused, expired or cancelled, and why.
//!
//! The publisher keeps the ledger, under its lock: it queues, cancels and
//! adopts edits itself, and takes the audio thread's outcomes from the
//! request log when asked (see `LiveGraph::timed_status`). Bounded: the
//! oldest entry goes once [`LEDGER_CAPACITY`] are kept.

use std::collections::VecDeque;

use super::Committed;
use crate::control_request::{Outcome, Refusal, RequestId};
use crate::invention::request_log::LoggedBeat;

/// The timed edits a ledger remembers. One waits at a time, so this covers
/// a long run of them between two reads.
const LEDGER_CAPACITY: usize = 32;

/// Where a timed edit stands.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EditState {
    /// Waiting for its time.
    Queued,
    /// The audio thread installed it; the runtime has yet to adopt it.
    Landed,
    /// Installed and adopted: the document, the snapshot and the built
    /// modules' handles describe it.
    Adopted,
    /// The audio thread refused it.
    Refused(Refusal),
    /// Its ttl passed before its time came.
    Expired,
    /// Another change cancelled it before it landed.
    Cancelled(CancelCause),
    /// Its outcome was lost (the outcome queue overflowed); it did not
    /// land, or it would have been adopted.
    Lost,
}

impl EditState {
    /// Whether it can still change: a lost outcome may yet turn up.
    fn open(&self) -> bool {
        matches!(self, Self::Queued | Self::Landed | Self::Lost)
    }
}

/// Why a timed edit was cancelled while it waited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CancelCause {
    /// Another change to the graph or the document committed first: a
    /// structural edit, a swap, a reload or an edit batch.
    Commit,
    /// An authored write to a control this edit `touches`: one of a module
    /// it removes or rebuilds, or one it writes as it lands.
    AuthoredWrite {
        module_id: String,
        key: String,
        touches: Touched,
    },
    /// A schedule written to a scheduler whose schedule this edit
    /// re-resolves, or which writes a module this edit rebuilds or removes.
    ScheduleWrite { module_id: String },
    /// The request queue had no room for the control writes it chains as
    /// it lands; the caller was refused (`QueueFull`).
    Unchained,
}

/// What a timed edit does to a control an authored write cancelled it for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Touched {
    /// It removes the control's module.
    Removed,
    /// It rebuilds the control's module.
    Rebuilt,
    /// It writes the control as it lands.
    Written,
}

/// A timed edit's state, with where it landed or will land.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditStatus {
    pub(crate) id: RequestId,
    pub(crate) state: EditState,
    /// The sample it lands on: known when queued for a sample, otherwise
    /// once it lands.
    pub(crate) sample: Option<u64>,
    /// The sample it was due on, when it landed later than that.
    pub(crate) due: Option<u64>,
    /// Its musical time, when it was timed in beats.
    pub(crate) beat: Option<LoggedBeat>,
    /// Modules it built (added or rebuilt), once adopted.
    pub(crate) built: Vec<String>,
    /// Modules it removed (not rebuilt), once adopted.
    pub(crate) removed: Vec<String>,
}

struct Entry {
    status: EditStatus,
    /// What adopting it committed: the built modules' handles, until taken.
    committed: Option<Committed>,
}

/// Every recent timed edit's status, and which changed since last taken.
#[derive(Default)]
pub(crate) struct Ledger {
    entries: VecDeque<Entry>,
    changed: VecDeque<RequestId>,
}

impl Ledger {
    /// Records edit `id`, just queued for `sample` or `beat`.
    pub(crate) fn queued(&mut self, id: RequestId, sample: Option<u64>, beat: Option<LoggedBeat>) {
        if self.entries.len() == LEDGER_CAPACITY {
            if let Some(oldest) = self.entries.pop_front() {
                self.changed.retain(|changed| *changed != oldest.status.id);
            }
        }
        self.entries.push_back(Entry {
            status: EditStatus {
                id,
                state: EditState::Queued,
                sample,
                due: None,
                beat,
                built: Vec::new(),
                removed: Vec::new(),
            },
            committed: None,
        });
        self.touch(id);
    }

    /// Records the audio thread's outcome for edit `id`. One cancelled or
    /// adopted keeps its state (a cancelled edit is refused `TargetGone`
    /// once due), taking only the sample it landed on, quietly: its final
    /// state was announced once, and a read shows the sample.
    pub(crate) fn outcome(&mut self, id: RequestId, outcome: Outcome) {
        let Some(status) = self.status_mut(id) else {
            return;
        };
        let open = status.state.open();
        match outcome {
            Outcome::Applied { at } | Outcome::AppliedLate { at, .. } => {
                status.sample = Some(at);
                if let Outcome::AppliedLate { due, .. } = outcome {
                    status.due = Some(due);
                }
                if !open {
                    return;
                }
                status.state = EditState::Landed;
            }
            Outcome::Refused(Refusal::Expired) if open => status.state = EditState::Expired,
            Outcome::Refused(refusal) if open => status.state = EditState::Refused(refusal),
            _ => return,
        }
        self.touch(id);
    }

    /// Records edit `id` cancelled by `cause`.
    pub(crate) fn cancelled(&mut self, id: RequestId, cause: CancelCause) {
        if let Some(status) = self.status_mut(id) {
            status.state = EditState::Cancelled(cause);
            self.touch(id);
        }
    }

    /// Records edit `id` adopted, keeping what it committed.
    pub(crate) fn adopted(&mut self, id: RequestId, committed: Committed) {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.status.id == id) else {
            return;
        };
        let status = &mut entry.status;
        status.state = EditState::Adopted;
        status.built = committed
            .started
            .iter()
            .map(|info| info.id.clone())
            .collect();
        let built = &status.built;
        let removed = committed.stopped.iter().filter(|id| !built.contains(id));
        status.removed = removed.cloned().collect();
        entry.committed = Some(committed);
        self.touch(id);
    }

    /// Marks lost every edit still queued but `waiting`, the one the
    /// publisher still waits for: outcomes were dropped, and theirs may be
    /// among them.
    pub(crate) fn lost(&mut self, waiting: Option<RequestId>) {
        let mut lost = Vec::new();
        for entry in &mut self.entries {
            let status = &mut entry.status;
            if status.state == EditState::Queued && Some(status.id) != waiting {
                status.state = EditState::Lost;
                lost.push(status.id);
            }
        }
        for id in lost {
            self.touch(id);
        }
    }

    pub(crate) fn status(&self, id: RequestId) -> Option<EditStatus> {
        self.entries
            .iter()
            .find(|entry| entry.status.id == id)
            .map(|entry| entry.status.clone())
    }

    /// The status of every edit that changed since the last call, in the
    /// order they last changed.
    pub(crate) fn take_changes(&mut self) -> Vec<EditStatus> {
        let changed = std::mem::take(&mut self.changed);
        changed
            .into_iter()
            .filter_map(|id| self.status(id))
            .collect()
    }

    /// What adopting edit `id` committed, once: its built modules' handles.
    pub(crate) fn take_committed(&mut self, id: RequestId) -> Option<Committed> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.status.id == id)?;
        entry.committed.take()
    }

    fn status_mut(&mut self, id: RequestId) -> Option<&mut EditStatus> {
        self.entries
            .iter_mut()
            .find(|entry| entry.status.id == id)
            .map(|entry| &mut entry.status)
    }

    fn touch(&mut self, id: RequestId) {
        self.changed.retain(|changed| *changed != id);
        self.changed.push_back(id);
    }
}
