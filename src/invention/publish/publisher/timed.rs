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

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::{Published, Publisher, Refused};
use crate::control_request::{QueueFull, Request, RequestId, RequestValue, When};
use crate::invention::declared::RequestPort;
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

/// How many recent adoptions [`Adoptions`] remembers.
const ADOPTIONS: usize = 8;

/// What the latest timed edits adopted did, for an authored write one may
/// have overtaken (see [`Publisher::timed_overtook`]).
#[derive(Default)]
pub(crate) struct Adoptions {
    recent: VecDeque<Adoption>,
    /// The generation of the newest adoption forgotten, 0 before any.
    forgotten: u64,
}

struct Adoption {
    /// The generation it was adopted as.
    generation: u64,
    id: RequestId,
    /// The modules it rebuilt or removed, or whose schedule it re-resolved
    /// as it landed.
    modules: HashSet<String>,
    /// The controls of modules it kept that it wrote as it landed.
    controls: Vec<(String, String)>,
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
        let removed = previous
            .modules
            .keys()
            .filter(|id| !self.mirror.modules.contains_key(*id));
        let built = waiting.built.keys();
        let rebound = waiting.packed.rebound.iter();
        let modules = removed.chain(built).chain(rebound).cloned().collect();
        self.adopted.recent.push_back(Adoption {
            generation: self.generation,
            id: waiting.id,
            modules,
            controls: waiting.chained,
        });
        if self.adopted.recent.len() > ADOPTIONS {
            if let Some(oldest) = self.adopted.recent.pop_front() {
                self.adopted.forgotten = oldest.generation;
            }
        }
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
            self.ledger.cancelled(id, cause());
        } else if waiting.fate.landed() {
            return false;
        }
        self.set_timed(None);
        true
    }

    /// Whether a timed edit adopted after generation `seen` may have
    /// overtaken an authored write to control `key` of `module_id` queued
    /// at `seen`: landing after it, the edit rebuilt or removed the module,
    /// re-resolved its schedule, or wrote the control. Not by writing the
    /// control, though, when it is `safe`: the edit the write was queued
    /// after it landed (see [`Self::timed_safe`]). True, to be safe, when
    /// one adopted since is forgotten.
    pub(crate) fn timed_overtook(
        &self,
        seen: u64,
        (module_id, key): (&str, &str),
        safe: Option<RequestId>,
    ) -> bool {
        let wrote = |(module, control): &(String, String)| module == module_id && control == key;
        let overtook = |adoption: &Adoption| {
            adoption.generation > seen
                && (adoption.modules.contains(module_id)
                    || (Some(adoption.id) != safe && adoption.controls.iter().any(wrote)))
        };
        self.adopted.forgotten > seen || self.adopted.recent.iter().any(overtook)
    }

    /// The id of the timed edit waiting, if any.
    pub(crate) fn timed_id(&self) -> Option<RequestId> {
        self.timed.as_ref().map(|waiting| waiting.id)
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
    /// writes as it lands, so an authored write to one cancels it (see
    /// `LiveGraph::record_authored`).
    pub(crate) fn chain_timed(&mut self, chained: Vec<(String, String)>) {
        if let Some(waiting) = self.timed.as_mut() {
            waiting.chained = chained;
        }
    }

    /// The timed edit an authored write about to be queued is safe from
    /// (see [`Self::timed_overtook`]): the one waiting, once it has landed,
    /// as the write follows its writes.
    pub(crate) fn timed_safe(&self) -> Option<RequestId> {
        let waiting = self.timed.as_ref()?;
        (waiting.fate.settled() == Some(true)).then_some(waiting.id)
    }

    /// Every recent timed edit's status (see `timed_status`).
    pub(crate) fn ledger(&mut self) -> &mut Ledger {
        &mut self.ledger
    }
}
