//! The publisher's side of a timed structural edit (see
//! `graph::publication::landing`): queued as a request with a `when`, it
//! leaves the publisher at the generation it was prepared against until
//! the audio thread lands it, and is adopted then.
//!
//! One timed edit waits at a time. Any other change publishing while it
//! waits cancels it: that change was prepared against the graph that is
//! playing, so it stands, and the timed edit is refused. One that finds
//! the edit landed and not yet adopted is refused instead (`TopologyMoved`,
//! nothing changed), as any change prepared against a generation that has
//! moved is.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{Published, Publisher, Refused};
use crate::control_request::{
    Condition, ControlTarget, Intent, QueueFull, Request, RequestId, RequestSender, RequestValue,
    RtValue, When,
};
use crate::invention::declared::{RequestPort, WriteOptions};
use crate::invention::graph::{Fate, TIMED_EDIT_MODULE};
use crate::invention::publish::change::{BuiltModule, PreparedChange, TopologyMirror};
use crate::invention::publish::rebind::Packed;
use crate::invention::publish::timed_status::{CancelCause, Ledger};
use crate::invention::runtime::GraphCommandError;
use crate::invention::state::RuntimeState;
use crate::modules::control_scheduler::Written;
use crate::payload::Payload;
use crate::ControlValue;
use indexmap::IndexMap;

/// Runs on the runtime state as a timed edit is adopted: records what it
/// changed in the document, as it applies.
pub(crate) type Retain = Box<dyn FnOnce(&mut RuntimeState) + Send>;

/// A timed edit queued and not yet adopted.
pub(crate) struct Waiting {
    id: RequestId,
    fate: Fate,
    /// The topology once it lands.
    mirror: TopologyMirror,
    built: IndexMap<String, BuiltModule>,
    retain: Retain,
    /// Where its built modules' surfaces submit once adopted.
    port: RequestPort,
    /// The schedules it hands surviving schedulers as it lands, recorded
    /// once it is adopted; a refused one records nothing.
    packed: Packed,
    /// Authored control writes made while it waits to modules it keeps,
    /// recorded again once `retain` has run (see [`Publisher::fold_timed`]).
    folded: Vec<(String, String, ControlValue)>,
    /// The controls of modules it keeps that it writes as it lands (see
    /// [`Publisher::chain_timed`]).
    chained: Vec<(String, String)>,
}

/// A timed edit the audio thread landed, adopted by the publisher: what it
/// replaced and built, for the runtime's other mirrors.
pub(crate) struct Adopted {
    pub(crate) id: RequestId,
    pub(crate) published: Published,
    pub(crate) retain: Retain,
    pub(crate) port: RequestPort,
    /// The schedules it handed surviving schedulers, to record now.
    pub(crate) written: Vec<Written>,
    /// Authored writes folded into it, to record after `retain`.
    pub(crate) folded: Vec<(String, String, ControlValue)>,
    /// The controls of modules it kept that it wrote as it landed.
    pub(crate) chained: Vec<(String, String)>,
}

impl Publisher {
    /// Queues `prepared` to land on the audio thread `when` (any `when`
    /// but `Now`), refused after `ttl` samples, and returns the request's
    /// id. `packed` is what `rebind::pack` packed into it, kept until it
    /// is adopted. Refused with nothing queued as [`Self::publish`]
    /// refuses a change, when the change edits nothing, or while another
    /// timed edit waits (`ControlError`).
    ///
    /// `before` runs once every check has passed, with the id the edit is
    /// about to be queued with: requests submitted there that wait on it
    /// applying are queued ahead of it, so none can miss it. Refused, with
    /// the edit not queued, when `before` refuses (having queued nothing).
    #[allow(clippy::result_large_err)]
    pub(crate) fn publish_timed(
        &mut self,
        mut prepared: PreparedChange,
        when: When,
        ttl: Option<u64>,
        (retain, port, packed): (Retain, RequestPort, Packed),
        before: impl FnOnce(RequestId) -> Result<(), GraphCommandError>,
    ) -> Result<RequestId, Refused> {
        let refuse = |error, change| Err(Refused { error, change });
        let control = |message: &str| GraphCommandError::ControlError(message.into());
        if prepared.base_generation != self.generation {
            return refuse(GraphCommandError::TopologyMoved, prepared);
        }
        if !self.audio_alive() {
            return refuse(GraphCommandError::AudioThreadStopped, prepared);
        }
        if self.timed.is_some() {
            let waiting = "A timed edit already waits; try again once it lands";
            return refuse(control(waiting), prepared);
        }
        if prepared.publication.is_none() {
            return refuse(control("Only a structural edit can be timed"), prepared);
        }
        if !self.requests.has_room() {
            return refuse(GraphCommandError::QueueFull, prepared);
        }
        let id = self.requests.take_id();
        if let Err(error) = before(id) {
            return refuse(error, prepared);
        }
        let mut publication = prepared.publication.take().expect("checked above");
        publication.map_survivors(self.mirror.modules.keys().map(String::as_str));
        publication.generation = self.generation + 1;
        let fate = Fate::default();
        publication.fate = Some(fate.guard());
        let mut edit = Request::edit(self.generation, Payload::owned(publication));
        edit.target.module_idx = TIMED_EDIT_MODULE;
        edit.when = when;
        edit.ttl = ttl;
        // `before` left room for it while submissions are serialized, as
        // they are under the publisher.
        let id = match self.requests.submit_as(id, edit) {
            Ok(id) => id,
            Err(QueueFull(edit)) => {
                let RequestValue::Edit(payload) = edit.value else {
                    unreachable!("the queue hands back the request it was given");
                };
                prepared.publication = payload.take_owned().ok();
                return refuse(GraphCommandError::QueueFull, prepared);
            }
        };
        self.set_timed(Some(Waiting {
            id,
            fate,
            mirror: prepared.mirror,
            built: prepared.built,
            retain,
            port,
            packed,
            folded: Vec::new(),
            chained: Vec::new(),
        }));
        Ok(id)
    }

    /// Sets or clears the timed edit waiting, and the flag mirroring it.
    fn set_timed(&mut self, waiting: Option<Waiting>) {
        // A hint for the reclaimer only: whatever acts on a timed edit
        // takes the publisher lock, so `Relaxed` is enough. A stale `false`
        // delays adoption to the next tick or the next lock.
        self.timed_flag.store(waiting.is_some(), Ordering::Relaxed);
        self.timed = waiting;
    }

    /// Whether a timed edit waits, readable without the publisher lock:
    /// a hint, as [`Self::set_timed`] says.
    pub(crate) fn timed_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.timed_flag)
    }

    /// Whether a timed edit waits: queued, and neither adopted nor known
    /// to be refused.
    pub(crate) fn timed_waiting(&self) -> bool {
        self.timed.is_some()
    }

    /// Adopts the timed edit the audio thread has landed, as
    /// [`Self::publish`] adopts a change: the next generation, with its
    /// mirror. Forgets one that was refused or expired. Call it before
    /// anything is resolved against the mirror.
    pub(crate) fn adopt(&mut self) -> Option<Adopted> {
        // One read: a second could see it land after the first missed it.
        match self.timed.as_ref()?.fate.settled() {
            None => return None,
            Some(false) => {
                self.set_timed(None);
                return None;
            }
            Some(true) => {}
        }
        let waiting = self.timed.take()?;
        self.set_timed(None); // for the flag: `take` cleared `timed`
        self.generation += 1;
        // Releases every request and write queued before it to the audio
        // thread, which then knows nothing is tagged older (see `landing`).
        self.published.store(self.generation, Ordering::Release);
        let previous = std::mem::replace(&mut self.mirror, waiting.mirror);
        Some(Adopted {
            id: waiting.id,
            published: Published {
                previous,
                built: waiting.built,
            },
            retain: waiting.retain,
            port: waiting.port,
            written: waiting.packed.written,
            folded: waiting.folded,
            chained: waiting.chained,
        })
    }

    /// Whether the timed edit waiting, if any, races a payload write to
    /// `module_id` resolved against the graph that is playing to write
    /// `targets` (a schedule): it re-resolved the schedule the write
    /// replaces, or it rebuilds or removes a module the write would go on
    /// writing. Such a write publishes as a change does: it cancels the
    /// edit ([`Self::cancel_timed`]), or is stale once the edit has landed.
    pub(crate) fn timed_races(&self, module_id: &str, targets: &[String]) -> bool {
        self.timed
            .as_ref()
            .is_some_and(|waiting| waiting.packed.races(module_id, targets))
    }

    /// Clears the way for a change publishing now: cancels a timed edit
    /// still waiting, recording `cause` (see [`Self::ledger`]). False when
    /// it has landed and is not yet adopted: the change was prepared
    /// against a generation that has moved.
    pub(crate) fn cancel_timed(&mut self, cause: impl FnOnce() -> CancelCause) -> bool {
        let Some(waiting) = &self.timed else {
            return true;
        };
        if waiting.fate.cancel() {
            let id = waiting.id;
            self.requests.cancelled(id);
            self.ledger.cancelled(id, cause());
        } else if waiting.fate.landed() {
            return false;
        }
        self.set_timed(None);
        true
    }

    /// The id of the timed edit waiting, if any.
    pub(crate) fn timed_id(&self) -> Option<RequestId> {
        self.timed.as_ref().map(|waiting| waiting.id)
    }

    /// The timed edit waiting, when it has landed and is not yet adopted:
    /// a write queued now applies after its writes.
    pub(crate) fn timed_landed(&self) -> Option<RequestId> {
        let waiting = self.timed.as_ref()?;
        (waiting.fate.settled() == Some(true)).then_some(waiting.id)
    }

    /// What the timed edit waiting, if any, does to module `id`: removes
    /// it (`Some(true)`) or rebuilds it (`Some(false)`). `None` when it
    /// keeps it, or none waits.
    pub(crate) fn timed_replaces(&self, id: &str) -> Option<bool> {
        let waiting = self.timed.as_ref()?;
        if !self.mirror.modules.contains_key(id) {
            return None;
        }
        if !waiting.mirror.modules.contains_key(id) {
            return Some(true);
        }
        waiting.built.contains_key(id).then_some(false)
    }

    /// Folds an authored write to a module the timed edit waiting keeps
    /// into it, so its retained document records the write again as it is
    /// adopted rather than lose it. The last write to each control stands.
    /// Nothing when none waits.
    pub(crate) fn fold_timed(&mut self, module_id: &str, key: &str, value: &ControlValue) {
        let Some(waiting) = self.timed.as_mut() else {
            return;
        };
        let folded = &mut waiting.folded;
        folded.retain(|(module, control, _)| module != module_id || control != key);
        folded.push((module_id.to_string(), key.to_string(), value.clone()));
    }

    /// Whether the timed edit waiting writes control `key` of `module_id`
    /// as it lands (see [`Self::chain_timed`]).
    pub(crate) fn timed_chains(&self, module_id: &str, key: &str) -> bool {
        self.timed.as_ref().is_some_and(|waiting| {
            let chained =
                |(module, control): &(String, String)| module == module_id && control == key;
            waiting.chained.iter().any(chained)
        })
    }

    /// Notes the controls of modules it keeps that the timed edit waiting
    /// writes as it lands, so an authored write to one is kept over it
    /// (see [`Self::keep_authored`]).
    pub(crate) fn chain_timed(&mut self, chained: Vec<(String, String)>) {
        if let Some(waiting) = self.timed.as_mut() {
            waiting.chained = chained;
        }
    }

    /// Readies an authored write of `raw` to control `key` of `module_id`
    /// (at `target`, in the graph that is playing), about to be queued,
    /// against the timed edit waiting, and returns the edit it is safe from
    /// (one whose write to the control does not replace it, see
    /// `LiveGraph::reconcile_authored`) with the copy it queued, if any.
    /// One landed already: the write follows its writes. One that writes
    /// the control as it lands: the write is kept over it, a copy chained
    /// to the landing as the edit's own write is, at the write's rank, and
    /// to the write applying ([`Condition::kept`]).
    ///
    /// The write must then be queued with the copy's [`Kept::write`] id,
    /// straight after it. The audio thread meets requests in the order they
    /// are queued and applies those waiting on a condition in the order it
    /// received them, and at one sample, of equal rank, the later wins: the
    /// copy, queued after the edit and so after its write, plays at the
    /// landing, and the edit's value never sounds when the copy reaches the
    /// audio thread before the landing; held back past it, the copy plays
    /// as soon as it does (see [`Ledger::kept`]). A copy replaces the copy
    /// kept before it as it reaches the audio thread, holding one slot
    /// however long a drag. A write that loses at its own sample breaks its
    /// copy, so a value that never played is not re-asserted as the edit
    /// lands. The copy carries the write's intent and ttl, so its hold is
    /// the author's own, refusing no later write a plain one would not, and
    /// it expires as the write would.
    ///
    /// `None`, with nothing queued, when neither: none waits, it does not
    /// write the control, or the write cannot be kept (an event, the
    /// requests waiting on conditions are full, or `requests`, the sender
    /// the write goes through, has no room for both), and an edit writing
    /// the control is cancelled as the write is recorded.
    pub(crate) fn keep_authored(
        &mut self,
        (module_id, key): (&str, &str),
        target: ControlTarget,
        (raw, event, options): (RtValue, bool, &WriteOptions),
        requests: &RequestSender,
    ) -> Option<(RequestId, Option<Kept>)> {
        let waiting = self.timed.as_ref()?;
        match waiting.fate.settled() {
            Some(true) => return Some((waiting.id, None)),
            Some(false) => return None,
            None => {}
        }
        let chained = |(module, control): &(String, String)| module == module_id && control == key;
        if event || !waiting.chained.iter().any(chained) {
            return None;
        }
        // Room for the write itself after it, through the same sender (and
        // its reserve), so a kept write is always made.
        if requests.watchers_full() || !requests.has_room_for(2) {
            return None;
        }
        let id = waiting.id;
        let write = requests.take_id();
        let mut kept = Request::new(target, RequestValue::Value(raw));
        kept.intent = Intent::Author;
        kept.source = options.source;
        kept.priority = options.priority;
        kept.ttl = options.ttl;
        kept.when = kept_until(id, write);
        let copy = requests.submit(kept).ok()?;
        self.ledger.kept(id, format!("{module_id}.{key}"));
        Some((id, Some(Kept { copy, write })))
    }

    /// Every recent timed edit's status (see `timed_status`).
    pub(crate) fn ledger(&mut self) -> &mut Ledger {
        &mut self.ledger
    }
}

/// A write kept over a timed edit (see [`Publisher::keep_authored`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Kept {
    /// The copy, applying as the edit lands.
    pub(crate) copy: RequestId,
    /// The id the write must be queued with, which the copy waits on.
    pub(crate) write: RequestId,
}

/// When `write`, kept over timed edit `edit`, is copied to apply: as the
/// edit lands, if `write` applied.
pub(crate) fn kept_until(edit: RequestId, write: RequestId) -> When {
    When::Condition(Condition::kept(edit, write))
}
