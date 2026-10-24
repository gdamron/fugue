//! Authored control writes recorded in the retained document as the audio
//! thread applies them, not as they are made: a write refused, lost to a
//! higher rank or replaced at its sample never reaches the document, which
//! so always describes what plays.
//!
//! Outcomes arrive in the order the audio thread reported them, timed
//! edits' among them (see `PendingLog::take_steps`). The publisher takes
//! them in that order, adopting a timed edit that landed before recording
//! a write applied after it: so a write applied to a module an edit then
//! rebuilt is recorded, and then replaced by the edit's document, as it was
//! replaced in what plays.

use std::sync::{Mutex, PoisonError};

use super::timed::adopt_into;
use super::timed_status::{CancelCause, Touched};
use super::{Authoring, DocumentStep, LiveGraph, Mirrors, PendingLog, Publisher};
use crate::control_request::RequestId;
use crate::invention::audio_assets;
use crate::invention::state::RuntimeState;

impl LiveGraph {
    /// Catches the retained document up with the audio thread, under the
    /// publisher (see the module docs).
    pub(super) fn catch_up(&self, publisher: &mut Publisher) {
        let mirrors = (&*self.control_surfaces, &*self.module_ports, &*self.state);
        catch_up(publisher, &self.pending, mirrors);
    }

    /// Readies the timed edit waiting, if any, for an authored write to
    /// control `key` of `module_id` just made, `safe` from the edit it
    /// names (see `declared::Queued`), and records `now`, when the write
    /// applied as it was made rather than as a request (building, a legacy
    /// control, a payload travelling in a publication).
    ///
    /// A timed edit waiting that rebuilds or removes the module is
    /// cancelled, and the edit reports why, as is one that writes the
    /// control as it lands but did not keep the write over its own (see
    /// `Publisher::keep_authored`): the write may reach the audio thread
    /// before or after the edit lands, so only that makes the write what
    /// plays. One landed already is adopted: the write's outcome then says
    /// whether it plays (applied to the graph the edit built, or refused
    /// with the module it replaced).
    pub(crate) fn reconcile_authored(
        &self,
        (module_id, key): (&str, &str),
        safe: Option<RequestId>,
        now: Option<&Authoring>,
    ) {
        let mut publisher = self.lock();
        let touches = match publisher.timed_replaces(module_id) {
            Some(true) => Some(Touched::Removed),
            Some(false) => Some(Touched::Rebuilt),
            None => (publisher.timed_chains(module_id, key) && publisher.timed_id() != safe)
                .then_some(Touched::Written),
        };
        if let Some(touches) = touches {
            let cause = || CancelCause::AuthoredWrite {
                module_id: module_id.to_string(),
                key: key.to_string(),
                touches,
            };
            if !publisher.cancel_timed(cause) {
                self.catch_up(&mut publisher);
            }
        }
        if let Some(authoring) = now {
            record(&mut publisher, &self.state, (module_id, key), authoring);
        }
    }
}

/// Takes every step the pending log lists, in order, then adopts a timed
/// edit that has landed since (its outcome not yet received, so every
/// write listed applied before it).
pub(super) fn catch_up(
    publisher: &mut Publisher,
    pending: &Mutex<PendingLog>,
    mirrors: Mirrors<'_>,
) {
    let steps = pending
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take_steps();
    for step in steps {
        match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } => record(publisher, mirrors.2, (&module_id, &key), &authoring),
            DocumentStep::Timed => adopt_into(publisher, mirrors),
        }
    }
    adopt_into(publisher, mirrors);
}

/// Records an authored write that applied, under the publisher: in the
/// retained document and the module's stored config, and in the document
/// of a timed edit waiting that keeps its module and does not write the
/// control as it lands, which records it again as it is adopted rather
/// than lose it: the write plays on past the landing. One the edit
/// rebuilds or writes is replaced as it lands, in what plays and in the
/// document both (a write kept over the edit's is recorded again as its
/// copy applies, see `Publisher::keep_authored`). A relative asset path is recorded
/// as it names, from the retained document's directory, the file it named
/// from the one it was resolved against (see
/// `audio_assets::rebase_asset_write`).
fn record(
    publisher: &mut Publisher,
    state: &Mutex<RuntimeState>,
    (module_id, key): (&str, &str),
    authoring: &Authoring,
) {
    let mut state = state.lock().unwrap();
    let origin = authoring.origin.as_deref();
    let value =
        audio_assets::rebase_asset_write(origin, state.source_path(), key, &authoring.value);
    if publisher.timed_replaces(module_id).is_none() && !publisher.timed_chains(module_id, key) {
        publisher.fold_timed(module_id, key, &value);
    }
    state.record_authored_control(module_id, key, &value);
}
