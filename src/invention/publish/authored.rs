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
//!
//! A commit recording a document of its own (an untimed change, a reload,
//! an edit batch, a timed edit adopted) forgets the writes not yet
//! recorded that it replaced, to a module it rebuilt or removed or to a
//! control it wrote (see [`Fence`]): recorded after it, they would say
//! what no longer plays.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use super::timed::adopt_into;
use super::timed_status::{CancelCause, Touched};
use super::{Authoring, DocumentStep, LiveGraph, Mirrors, PendingLog, Publisher};
use crate::control_request::RequestId;
use crate::invention::audio_assets;
use crate::invention::state::RuntimeState;

/// The longest a caller waits for the audio thread to take what it was
/// handed: a device stream takes a write within a block or two, so a write
/// still waiting after this is held up by something other than the audio
/// thread's next callback (a stream stopped or stalled).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const APPLY_WAIT: Duration = Duration::from_millis(200);

impl LiveGraph {
    /// Waits, at most `bound`, until every authored write made so far has
    /// applied and been recorded, or been refused. Returns whether none is
    /// still waiting. A graph with no clock settles each write before the
    /// call making it returns, so it never waits.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn settle_authored(&self, bound: Duration) -> bool {
        let deadline = std::time::Instant::now() + bound;
        loop {
            self.adopt_landed();
            let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            if !pending.authoring() {
                return true;
            }
            drop(pending);
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

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
    let mut steps = steps;
    while let Some(step) = steps.pop_front() {
        match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } => record(publisher, mirrors.2, (&module_id, &key), &authoring),
            DocumentStep::Timed => adopt_into(publisher, (pending, mirrors), &mut steps),
        }
    }
    adopt_into(publisher, (pending, mirrors), &mut steps);
}

/// The authored writes a commit may replace, not yet recorded: each
/// control with what the retained document held for it before. A commit
/// records its own document (a reload's, an edit batch's, a timed edit's
/// as it is adopted), planned without them; one still to be recorded is
/// recorded on top of it, unless the commit rebuilt or removed its module
/// or wrote the control itself. Then what plays is the commit's: the write
/// applied, if at all, to the module replaced, or before the commit's own
/// write. So it is forgotten (see [`Fence::close`]).
pub(super) struct Fence(Vec<((String, String), Option<serde_json::Value>)>);

impl Fence {
    /// Every authored write waiting in `pending`, and every one listed in
    /// `later`, yet to be recorded, but those safe from the timed edit
    /// being adopted, `except` (a write kept over it, or made once it had
    /// landed, which follows its writes).
    pub(super) fn open(
        pending: &Mutex<PendingLog>,
        state: &Mutex<RuntimeState>,
        (later, except): (&VecDeque<DocumentStep>, Option<RequestId>),
    ) -> Self {
        let mut keys =
            (pending.lock().unwrap_or_else(PoisonError::into_inner)).authored_keys(except);
        keys.extend(later.iter().filter_map(|step| match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } if except.is_none() || authoring.safe != except => {
                Some((module_id.clone(), key.clone()))
            }
            _ => None,
        }));
        let state = state.lock().unwrap();
        Self(
            keys.into_iter()
                .map(|key| {
                    let held = held(&state, &key);
                    (key, held)
                })
                .collect(),
        )
    }

    /// Forgets, in `pending` and `later`, each write the commit replaced:
    /// to a module it `stopped` (rebuilt or removed), or to a control
    /// whose document value it changed.
    pub(super) fn close(
        self,
        (pending, state): (&Mutex<PendingLog>, &Mutex<RuntimeState>),
        stopped: &[String],
        (later, except): (&mut VecDeque<DocumentStep>, Option<RequestId>),
    ) {
        if self.0.is_empty() {
            return;
        }
        let replaced: Vec<_> = {
            let state = state.lock().unwrap();
            (self.0.into_iter())
                .filter(|(key, was)| stopped.contains(&key.0) || held(&state, key) != *was)
                .map(|(key, _)| key)
                .collect()
        };
        let is_replaced =
            |module_id: &str, key: &str| replaced.iter().any(|(m, k)| m == module_id && k == key);
        let safe = |authoring: &Authoring| except.is_some() && authoring.safe == except;
        later.retain(|step| match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } => safe(authoring) || !is_replaced(module_id, key),
            DocumentStep::Timed => true,
        });
        let mut pending = pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.forget_authored(is_replaced, except);
    }
}

/// What the retained document holds for control `key` of `module_id`.
fn held(state: &RuntimeState, (module_id, key): &(String, String)) -> Option<serde_json::Value> {
    let document = state.document.as_ref()?;
    let spec = document.modules.iter().find(|spec| spec.id == *module_id)?;
    spec.config.get(key).cloned()
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
