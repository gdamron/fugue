//! An offline render's timed edits, the control side: the one waiting, the
//! one landed and not yet adopted, and the ledger of what became of each,
//! kept with the render's timed writes (see `offline_timed`) under the
//! graph's lock, where its control calls settle them.
//!
//! A timed edit is prepared and queued on the control thread (see
//! `publish::offline_timed`) as a request with a `when`, through the
//! render's own queue, so the drain takes it on the sample or beat a live
//! graph's drain would. The graph then holds what lands, as a live link
//! does, and installs it at that segment start (see
//! `publication::offline_landing`). What became of it is received in order
//! with the writes' outcomes, so an authored write that applied before it
//! is recorded before it is adopted, and one after it after.
//!
//! One waits at a time, as on a live graph. Any other edit installed while
//! it waits cancels it, as do a load or a reset (see `CancelCause`); a
//! render that fails strands it, refused `Stopped`.

use super::ended::EndGates;
use super::{Fate, Publication, SignalGraph};
use crate::control_request::{Outcome, Refusal, RequestId};
use crate::invention::publish::{CancelCause, Ledger, OfflineAdoption};
use crate::invention::request_log::LoggedBeat;

/// A timed edit queued on an offline render, with what adopting it
/// commits.
pub(crate) struct WaitingEdit {
    pub(crate) id: RequestId,
    pub(crate) fate: Fate,
    pub(crate) adoption: OfflineAdoption,
    /// The request generation it was queued against, which it replaces as
    /// it lands (see `SignalGraph::request_generation`).
    pub(crate) replaces: u64,
}

/// A timed edit the render landed, to adopt as the render settles: after
/// the first `before` authored writes settled with it, which applied
/// before it, and before the rest.
pub(crate) struct LandedEdit {
    pub(crate) before: usize,
    pub(crate) edit: WaitingEdit,
}

/// An offline render's timed edits (see the module docs).
#[derive(Default)]
pub(crate) struct OfflineEdits {
    /// The timed edit the render took this segment start, until it
    /// installs. Render side.
    pub(super) landing: Option<Box<Publication>>,
    /// What the last landing displaced, freed as the render settles.
    /// Render side.
    pub(super) displaced: Option<Box<Publication>>,
    /// The `ended` gates of the graph the waiting edit installs, which the
    /// render's end watch takes as it lands; afterwards those it replaced.
    pub(super) gates: Option<EndGates>,
    waiting: Option<WaitingEdit>,
    landed: Option<LandedEdit>,
    /// What became of each recent timed edit.
    pub(crate) ledger: Ledger,
    /// What is left to free off the graph's lock, as the render settles.
    pub(super) freed: FreedEdits,
}

/// What an offline render's timed edits leave to free once its lock is
/// released: a landing's displaced graph and gates, an edit that will not
/// land.
// A displaced graph is freed boxed, as the render kept it.
#[allow(clippy::vec_box)]
#[derive(Default)]
pub(crate) struct FreedEdits(Vec<Box<Publication>>, Vec<EndGates>, Vec<WaitingEdit>);

impl OfflineEdits {
    /// The timed edit waiting, if any.
    pub(crate) fn waiting(&self) -> Option<&WaitingEdit> {
        self.waiting.as_ref()
    }

    /// Waits on `edit`, just queued for `sample` or `beat`, whose graph has
    /// `gates`. Nothing else waits.
    pub(crate) fn queued(
        &mut self,
        edit: WaitingEdit,
        gates: EndGates,
        (sample, beat): (Option<u64>, Option<LoggedBeat>),
    ) {
        debug_assert!(self.waiting.is_none(), "one timed edit waits at a time");
        self.ledger.queued(edit.id, sample, beat);
        self.gates = Some(gates);
        self.waiting = Some(edit);
    }

    /// Takes the drain's `outcome` for request `id`, settled after
    /// `before` authored writes: the waiting edit landed, to adopt, or will
    /// never land. One cancelled since keeps its state (see
    /// [`Ledger::outcome`]).
    pub(super) fn outcome(&mut self, id: RequestId, outcome: Outcome, before: usize) {
        let applied = matches!(
            outcome,
            Outcome::Applied { .. } | Outcome::AppliedLate { .. }
        );
        self.ledger.outcome(id, outcome);
        let Some(edit) = self.waiting.take_if(|edit| edit.id == id) else {
            return;
        };
        self.freed.1.extend(self.gates.take());
        if applied {
            self.freed.0.extend(self.displaced.take());
            self.landed = Some(LandedEdit { before, edit });
        } else {
            self.freed.2.push(edit);
        }
    }

    /// Cancels the waiting edit for `cause`, unless it has landed (its
    /// outcome not yet settled) or nothing waits. Returns its id if it did.
    /// The drain is told by
    /// [`OfflineRequests::cancel_edit`](super::offline_timed::OfflineRequests::cancel_edit).
    pub(super) fn cancel(&mut self, cause: impl FnOnce() -> CancelCause) -> Option<RequestId> {
        let edit = self.waiting.take_if(|edit| edit.fate.cancel())?;
        let id = edit.id;
        self.ledger.cancelled(id, cause());
        self.free(edit);
        Some(id)
    }

    /// Refuses the waiting edit `Stopped`: the render failed, so it never
    /// lands.
    pub(super) fn strand(&mut self) {
        // The render will not take its landing, nor land another.
        self.freed.0.extend(self.landing.take());
        self.freed.0.extend(self.displaced.take());
        if let Some(edit) = self.waiting.take() {
            (self.ledger).outcome(edit.id, Outcome::Refused(Refusal::Stopped));
            self.free(edit);
        }
    }

    /// Frees `edit`, which will never land, and its gates, off the lock.
    fn free(&mut self, edit: WaitingEdit) {
        self.freed.1.extend(self.gates.take());
        self.freed.2.push(edit);
    }

    /// What is left to free, taken to free off the graph's lock.
    pub(super) fn take_freed(&mut self) -> FreedEdits {
        std::mem::take(&mut self.freed)
    }

    /// The edit landed and not yet adopted, taken to adopt.
    pub(super) fn take_landed(&mut self) -> Option<LandedEdit> {
        self.landed.take()
    }
}

impl SignalGraph {
    /// Whether an offline render landed a timed edit that has yet to be
    /// adopted, once what the render settled is received. Control thread.
    pub(crate) fn offline_edit_landed(&mut self) -> bool {
        let Some(offline) = self.offline.as_mut() else {
            return false;
        };
        offline.settle();
        offline.edits.landed.is_some()
    }

    /// Cancels an offline render's waiting timed edit for `cause`, once
    /// what the render settled is received: none lands after the change
    /// about to install. Control thread.
    pub(crate) fn cancel_offline_edit(&mut self, cause: impl FnOnce() -> CancelCause) {
        if let Some(offline) = self.offline.as_mut() {
            offline.settle();
            offline.cancel_edit(cause);
        }
    }
    /// Cancels an offline render's waiting timed edit, once what the render
    /// settled is received, when a schedule write to `module_id` resolved
    /// to write `targets` races it, as on a live graph (see
    /// [`OfflineAdoption::races`]). Returns false when it races and could
    /// not be cancelled; the write is stale then. Control thread.
    pub(crate) fn cancel_offline_edit_racing(
        &mut self,
        module_id: &str,
        targets: &[String],
    ) -> bool {
        let Some(offline) = self.offline.as_mut() else {
            return true;
        };
        offline.settle();
        let races =
            (offline.edits.waiting()).is_some_and(|edit| edit.adoption.races(module_id, targets));
        !races
            || offline.cancel_edit(|| CancelCause::ScheduleWrite {
                module_id: module_id.to_string(),
            })
    }
}
