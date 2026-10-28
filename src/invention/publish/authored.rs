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
//! what no longer plays. The writes recorded while its document was planned,
//! which that document lacks, are recorded again on top of it, but those it
//! replaced the same way (see [`Since`]).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use super::timed::adopt_into;
use super::timed_status::{CancelCause, Touched};
use super::{Authoring, DocumentStep, LiveGraph, Mirrors, PendingLog, PreparedChange, Publisher};
use crate::control_request::RequestId;
use crate::invention::reload::scalar_control_value;
use crate::invention::state::RuntimeState;
use crate::invention::{audio_assets, authored_document};
use crate::module_config::{names, replaced_by};
use crate::ControlValue;

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
    /// applied as it was made rather than as a request (building, an undeclared
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
        let mut cancelled = false;
        if let Some(touches) = touches {
            let cause = || CancelCause::AuthoredWrite {
                module_id: module_id.to_string(),
                key: key.to_string(),
                touches,
            };
            cancelled = publisher.cancel_timed(cause);
            if !cancelled {
                self.catch_up(&mut publisher);
            }
        }
        if let Some(authoring) = now {
            record(&mut publisher, &self.state, (module_id, key), authoring);
        }
        if cancelled {
            // A graph with no clock renders only when asked: what waits on
            // the edit is refused now, not at the next write, so nothing is
            // left listed pending meanwhile.
            drop(publisher);
            self.settle();
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
            } if !authoring.timed && (except.is_none() || authoring.safe != except) => {
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
    /// to a module it `stopped` (rebuilt or removed), to a control it
    /// `writes` (to any value, the one the document holds included), or to
    /// a control whose document value it changed otherwise. Each kept one
    /// is followed by the commit's writes it would hide, made after it
    /// (see [`overtaken`]), as the document holds them now.
    pub(super) fn close(
        self,
        (pending, state): (&Mutex<PendingLog>, &Mutex<RuntimeState>),
        (stopped, writes): (&[String], &[(String, String)]),
        (later, except): (&mut VecDeque<DocumentStep>, Option<RequestId>),
    ) {
        // A timed write the commit replaced only by rebuilding or removing
        // its module: one to a control the commit writes is ordered by when
        // each applies (see `Committed::contested`).
        if self.0.is_empty() && stopped.is_empty() {
            return;
        }
        let replaced: Vec<_> = {
            let state = state.lock().unwrap();
            (self.0.into_iter())
                .filter(|(key, was)| {
                    stopped.contains(&key.0) || writes.contains(key) || held(&state, key) != *was
                })
                .map(|(key, _)| key)
                .collect()
        };
        let is_replaced = |module_id: &str, key: &str, authoring: &Authoring| match authoring.timed
        {
            true => stopped.iter().any(|stopped| stopped == module_id),
            false => replaced.iter().any(|(m, k)| m == module_id && k == key),
        };
        let safe = |authoring: &Authoring| except.is_some() && authoring.safe == except;
        later.retain(|step| match step {
            DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } => safe(authoring) || !is_replaced(module_id, key, authoring),
            DocumentStep::Timed => true,
        });
        (pending.lock().unwrap_or_else(PoisonError::into_inner))
            .forget_authored(is_replaced, except);
        // Those kept, with the commit's writes each would hide, read with
        // the log unlocked (the state is locked first elsewhere).
        let kept = (pending.lock().unwrap_or_else(PoisonError::into_inner)).authored_keys(except);
        let follows: Vec<_> = {
            let state = state.lock().unwrap();
            let earlier = |(module_id, key): &(String, String)| {
                [(module_id.clone(), key.clone(), ControlValue::Bool(false))]
            };
            let later_keys = later.iter().filter_map(|step| match step {
                DocumentStep::Authored {
                    module_id,
                    key,
                    authoring,
                } if !safe(authoring) && !authoring.timed => Some((module_id.clone(), key.clone())),
                _ => None,
            });
            (kept.into_iter().chain(later_keys))
                .map(|written| {
                    let overtaken = overtaken(&state, &earlier(&written), writes);
                    let then: Vec<_> = (overtaken.into_iter())
                        .map(|(_, key, value)| (key, value))
                        .collect();
                    (written, then)
                })
                .filter(|(_, then)| !then.is_empty())
                .collect()
        };
        if follows.is_empty() {
            return;
        }
        let follows = |module_id: &str, key: &str| {
            (follows.iter())
                .find(|((m, k), _)| m == module_id && k == key)
                .map_or_else(Vec::new, |(_, then)| then.clone())
        };
        for step in later.iter_mut() {
            if let DocumentStep::Authored {
                module_id,
                key,
                authoring,
            } = step
            {
                if !safe(authoring) && !authoring.timed {
                    authoring.then.extend(follows(module_id, key));
                }
            }
        }
        (pending.lock().unwrap_or_else(PoisonError::into_inner)).follow_authored(follows, except);
    }
}

/// The authored writes the retained document recorded after a change was
/// planned from it, which the document it records lacks, oldest first.
pub(super) struct Since {
    writes: Vec<(String, String, ControlValue)>,
    /// The invention's path as they were recorded, which a relative asset
    /// path they name is relative to.
    origin: Option<PathBuf>,
}

impl Since {
    /// What `state` recorded since `prepared` was planned; `None` when it
    /// no longer keeps them all (see `RuntimeState::authored_log`).
    pub(super) fn of(prepared: &PreparedChange, state: &Mutex<RuntimeState>) -> Option<Self> {
        let state = state.lock().unwrap();
        let writes = match prepared.authored_at {
            Some(at) => state
                .authored_log
                .last(state.authored_epoch.checked_sub(at)?)?,
            None => Vec::new(),
        };
        let origin = state.source_path().map(Path::to_path_buf);
        Some(Self { writes, origin })
    }

    /// Records them again on top of the document the commit just retained,
    /// all but those it replaced: to a module it `stopped` (the module
    /// playing was built from its document) or a control it `writes` (its
    /// write follows). Before the commit's fence closes, so a write still
    /// to be recorded that follows one of them is kept.
    pub(super) fn rebase(
        self,
        state: &Mutex<RuntimeState>,
        (stopped, writes): (&[String], &[(String, String)]),
    ) {
        if self.writes.is_empty() {
            return;
        }
        let mut state = state.lock().unwrap();
        let replaced = |module_id: &str, key: &str| {
            stopped.iter().any(|m| m == module_id)
                || writes.iter().any(|(m, k)| m == module_id && k == key)
        };
        let mut earlier = self.writes;
        earlier.retain(|(module_id, key, _)| !replaced(module_id, key));
        let overtaken = overtaken(&state, &earlier, writes);
        for (module_id, key, value) in earlier {
            let (from, to) = (self.origin.as_deref(), state.source_path());
            let asset = state.names_audio(&module_id, &key);
            let value = audio_assets::rebase_asset_write(from, to, asset, &value);
            state.record_authored_control(&module_id, &key, &value);
        }
        // Then those of the commit's own they would hide, made after.
        for (module_id, key, value) in &overtaken {
            state.record_authored_control(module_id, key, value);
        }
    }

    /// Folds them into the document of the timed edit just queued, as a
    /// write recorded while it waits is (see `record`): all but those to a
    /// module it rebuilds or a control it writes as it lands.
    pub(super) fn fold_into(self, publisher: &mut Publisher) {
        for (module_id, key, value) in self.writes {
            let replaced = publisher.timed_replaces(&module_id).is_some();
            if !replaced && !publisher.timed_chains(&module_id, &key) {
                publisher.fold_timed(&module_id, &key, &value);
            }
        }
    }
}

/// The controls a change writes (`writes`) that applied after `earlier`
/// authored writes to their module where one of the two replaces the other
/// (see [`ConfigKey::replacing`](crate::module_config::ConfigKey::replacing)),
/// with the values `state`'s document holds for them: recorded again after
/// `earlier` is, so the document records them in the order they applied.
/// One no control takes is left as the document holds it.
pub(super) fn overtaken(
    state: &RuntimeState,
    earlier: &[(String, String, ControlValue)],
    writes: &[(String, String)],
) -> Vec<(String, String, ControlValue)> {
    let Some(document) = state.document.as_ref() else {
        return Vec::new();
    };
    let replaces = |module_id: &str, whole: &str, key: &str| {
        let Some(info) = state.modules.get(module_id) else {
            return false;
        };
        let keys = state.replacing.config_keys(&info.module_type);
        replaced_by(keys, whole)
            .iter()
            .any(|replaced| names(replaced, key))
    };
    let hidden = |module_id: &str, key: &str| {
        (earlier.iter())
            .any(|(m, k, _)| m == module_id && (replaces(m, k, key) || replaces(m, key, k)))
    };
    (writes.iter())
        .filter(|(module_id, key)| hidden(module_id, key))
        .filter_map(|(module_id, key)| {
            let held = authored_document::held_control(document, module_id, key)?;
            let value = scalar_control_value(&held)?;
            Some((module_id.clone(), key.clone(), value))
        })
        .collect()
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
/// document both (a timed one the edit writes, as the edit's write applies:
/// kept, should that lose its sample) (a write kept over the edit's is recorded again as its
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
    // A timed edit's own write, applied after the edit, changes nothing
    // unless a write recorded since has.
    if authoring.chained && state.holds(module_id, key, &authoring.value) {
        return;
    }
    let origin = authoring.origin.as_deref();
    let asset = state.names_audio(module_id, key);
    let value =
        audio_assets::rebase_asset_write(origin, state.source_path(), asset, &authoring.value);
    // A timed write is folded into a waiting edit's document even for a
    // control the edit writes as it lands: the edit's own write, recorded
    // as it applies, replaces it then, and leaves it should it never apply.
    let mut record = |key: &str, value: &ControlValue| {
        let chained = !authoring.timed && publisher.timed_chains(module_id, key);
        if publisher.timed_replaces(module_id).is_none() && !chained {
            publisher.fold_timed(module_id, key, value);
        }
        state.record_authored_control(module_id, key, value);
    };
    record(key, &value);
    // The commit's writes that applied after it, in the same order, the
    // timed edit's document included.
    for (key, value) in &authoring.then {
        record(key, value);
    }
}
