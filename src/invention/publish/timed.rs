//! Structural edits timed like control requests (B6): a prepared change
//! (an ApplyEdits batch, a reload, a swap) that lands on the sample its
//! time resolves to, rather than at the next block.
//!
//! The change is prepared and validated now, against the graph that is
//! playing, and queued as a request (see `graph::publication::landing`).
//! It applies only to that graph: any other change published before it
//! lands cancels it, and it is refused (`TargetGone`). Its document change
//! is recorded when it lands, as the publisher adopts it: at the next call
//! that takes the publisher.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use indexmap::IndexMap;

use super::change::BuiltModule;
use super::rebind;
use super::timed_status::EditStatus;
use super::watch::{condition_of, Part};
use super::{
    publisher, Committed, LiveGraph, Mirrors, PendingLog, PreparedChange, Publisher, Refused,
};
use crate::control_request::{BeatSpec, BeatTime, RequestId, When};
use crate::invention::orchestration::ModulePorts;
use crate::invention::request_log::LoggedBeat;
use crate::invention::runtime::ControlSurfaceInstance;
use crate::invention::runtime::GraphCommandError;
use crate::invention::state::RuntimeState;

/// When a timed edit lands, resolved against the graph that is playing.
#[cfg_attr(not(test), allow(dead_code))] // until the runtime calls `commit_when`
#[derive(Clone, Copy, Debug)]
pub(crate) enum EditTime<'a> {
    /// On this sample of the transport.
    AtSample(u64),
    /// This many samples from now.
    AfterSamples(u64),
    /// On clock `clock`'s beat `spec` (one beat, not a repeating grid):
    /// the sample the clock reaches it on.
    Beat { clock: &'a str, spec: BeatSpec },
    /// Once every one of `parts` has happened: on the sample when that is
    /// a segment start (another request applying, a control crossing),
    /// otherwise at the next segment start, reported late.
    Condition(&'a [Part<'a>]),
}

/// Module types whose scripts or agents a commit starts and stops, which
/// a timed edit cannot do yet as it lands.
#[cfg_attr(not(test), allow(dead_code))] // until the runtime calls `commit_when`
const FOLLOWED_UP: [&str; 2] = ["code", "agent"];

impl LiveGraph {
    /// Takes the publisher, first adopting a timed edit the audio thread
    /// has landed, so whatever is resolved under it sees the graph playing.
    pub(super) fn lock(&self) -> MutexGuard<'_, Publisher> {
        let mut publisher = self.publisher.lock().unwrap();
        self.adopt(&mut publisher);
        publisher
    }

    /// Commits the runtime mirrors and document for a timed edit the audio
    /// thread has landed, as [`Self::commit_with`] commits a change.
    fn adopt(&self, publisher: &mut Publisher) {
        let mirrors = (&*self.control_surfaces, &*self.module_ports, &*self.state);
        adopt_into(publisher, mirrors);
    }

    /// Adopts a timed edit the audio thread has landed, if any, so what is
    /// read next describes the graph that is playing.
    pub(crate) fn adopt_landed(&self) {
        drop(self.lock());
    }

    /// What adopts a landed timed edit from the reclaimer thread (see
    /// [`Adopter`]).
    pub(super) fn adopter(&self) -> Adopter {
        Adopter {
            waiting: self.publisher.lock().unwrap().timed_flag(),
            publisher: Arc::downgrade(&self.publisher),
            surfaces: Arc::downgrade(&self.control_surfaces),
            ports: Arc::downgrade(&self.module_ports),
            state: Arc::downgrade(&self.state),
        }
    }

    /// Where timed edit `id` stands, if the ledger still remembers it.
    #[cfg_attr(not(test), allow(dead_code))] // until the runtime reads timed edits' status
    pub(crate) fn timed_status(&self, id: RequestId) -> Option<EditStatus> {
        let mut publisher = self.lock();
        self.sync_timed(&mut publisher);
        publisher.ledger().status(id)
    }

    /// Every timed edit whose status changed since the last call, in the
    /// order they changed.
    #[cfg_attr(not(test), allow(dead_code))] // until the runtime reads timed edits' status
    pub(crate) fn take_timed_changes(&self) -> Vec<EditStatus> {
        let mut publisher = self.lock();
        self.sync_timed(&mut publisher);
        publisher.ledger().take_changes()
    }

    /// What adopting timed edit `id` committed (its built modules'
    /// handles), once it is adopted; taken once.
    #[cfg_attr(not(test), allow(dead_code))] // until the runtime reads timed edits' status
    pub(crate) fn take_timed_committed(&self, id: RequestId) -> Option<Committed> {
        self.lock().ledger().take_committed(id)
    }

    /// Takes the audio thread's outcomes for timed edits into the ledger.
    #[cfg_attr(not(test), allow(dead_code))] // until the runtime reads timed edits' status
    fn sync_timed(&self, publisher: &mut Publisher) {
        sync_ledger(publisher, &mut self.pending.lock().unwrap());
    }

    /// Queues `prepared` to land at `time`, refused once `ttl` samples have
    /// passed, and `retain` to run on the runtime state as it lands (a
    /// reload's document, say). Returns the request's id, whose outcome
    /// [`Self::take_outcomes`] reports: applied (at the sample it landed
    /// on), or refused when another change published first.
    ///
    /// Refused with nothing queued when `prepared` is stale or edits
    /// nothing, another timed edit waits, `time` names an unknown module or
    /// a part that can never be met, or the change adds or removes a module
    /// that runs a script or an agent (`ControlError`).
    #[cfg_attr(not(test), allow(dead_code))] // until the runtime calls `commit_when`
    pub(crate) fn commit_when(
        &self,
        mut prepared: PreparedChange,
        time: EditTime<'_>,
        ttl: Option<u64>,
        retain: impl FnOnce(&mut RuntimeState) + Send + 'static,
    ) -> Result<RequestId, GraphCommandError> {
        self.reclaim();
        let mut publisher = self.lock();
        if prepared.base_generation != publisher.generation() {
            drop(publisher);
            drop(prepared);
            return Err(GraphCommandError::TopologyMoved);
        }
        // Under the publisher, where a schedule write records itself, as a
        // commit packs them: each survivor re-resolves the schedule it will
        // be playing, handed over as the edit lands and recorded only once
        // it is adopted. A write racing it while it waits cancels it (see
        // `Publisher::timed_races`).
        let result = self
            .resolve_edit_time(&publisher, &prepared, time)
            .and_then(|when| Ok((when, rebind::pack(&mut prepared)?)));
        let (when, packed) = match result {
            Ok(resolved) => resolved,
            Err(error) => {
                drop(publisher);
                drop(prepared);
                return Err(error);
            }
        };
        let port = self.port();
        // Counted in the request log as it applies: registered under the
        // log, locked before the submission, as a write is (see `watch`).
        let mut pending = self.pending.lock().unwrap();
        pending.settle();
        match publisher.publish_timed(prepared, when, ttl, (Box::new(retain), port, packed)) {
            Ok(id) => {
                pending.timed(id);
                let (sample, beat) = landing_of(time);
                publisher.ledger().queued(id, sample, beat);
                drop(pending);
                drop(publisher);
                self.settle();
                Ok(id)
            }
            Err(Refused { error, change }) => {
                drop(pending);
                drop(publisher);
                drop(change);
                Err(error)
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))] // until the runtime calls `commit_when`
    fn resolve_edit_time(
        &self,
        publisher: &Publisher,
        prepared: &PreparedChange,
        time: EditTime<'_>,
    ) -> Result<When, GraphCommandError> {
        let control = GraphCommandError::ControlError;
        let state = self.state.lock().unwrap();
        let followed = |id: &String| {
            let module_type = state.modules.get(id).map(|info| info.module_type.as_str());
            module_type.is_some_and(|t| FOLLOWED_UP.contains(&t))
        };
        let mirror = publisher.mirror();
        let gone = mirror
            .modules
            .keys()
            .filter(|id| !prepared.mirror.modules.contains_key(*id));
        let builds = |module: &BuiltModule| FOLLOWED_UP.contains(&module.info.module_type.as_str());
        if gone.chain(prepared.built.keys()).any(followed) || prepared.built.values().any(builds) {
            let message = "A timed edit cannot add or remove a code or agent module yet";
            return Err(control(message.into()));
        }
        drop(state);
        Ok(match time {
            EditTime::AtSample(sample) => When::AtSample(sample),
            EditTime::AfterSamples(samples) => When::AfterSamples(samples),
            EditTime::Beat { clock, spec } => When::Beat(BeatTime {
                clock: publisher.beat_clock(clock, Some(clock))?,
                spec,
                ramp: None,
            }),
            EditTime::Condition(parts) => {
                if self.requests.watchers_full() {
                    let full = "Too many requests wait on a condition; try again once some apply";
                    return Err(control(full.into()));
                }
                let surfaces = self.control_surfaces.lock().unwrap();
                When::Condition(condition_of(mirror, &surfaces, parts)?)
            }
        })
    }
}

/// Adopts the timed edit the audio thread landed, if any, under
/// `publisher`: the publisher's side, then the runtime's mirrors and
/// document, as a commit does.
fn adopt_into(publisher: &mut Publisher, mirrors: Mirrors<'_>) {
    let Some(publisher::Adopted {
        id,
        published,
        retain,
        port,
        written,
    }) = publisher.adopt()
    else {
        return;
    };
    // Under the publisher, as a commit records them.
    for schedule in written {
        schedule.record();
    }
    let committed = LiveGraph::commit_mirrors(mirrors, publisher, published, &port, retain);
    publisher.ledger().adopted(id, committed);
}

/// Takes `pending`'s outcomes for timed edits into `publisher`'s ledger.
#[cfg_attr(not(test), allow(dead_code))] // until the runtime reads timed edits' status
fn sync_ledger(publisher: &mut Publisher, pending: &mut PendingLog) {
    let (outcomes, lost) = pending.take_timed();
    let waiting = publisher.timed_id();
    let ledger = publisher.ledger();
    for (id, outcome) in outcomes {
        ledger.outcome(id, outcome);
    }
    if lost {
        ledger.lost(waiting);
    }
}

/// Where a timed edit lands, as far as is known when it is queued: the
/// sample for one timed to a sample, the beat for one timed in beats.
fn landing_of(time: EditTime<'_>) -> (Option<u64>, Option<LoggedBeat>) {
    match time {
        EditTime::AtSample(sample) => (Some(sample), None),
        EditTime::Beat { clock, spec } => {
            let clock = clock.to_string();
            let beat = match spec {
                BeatSpec::After(beats) => LoggedBeat::After { clock, beats },
                BeatSpec::Grid {
                    every,
                    offset,
                    repeat,
                } => LoggedBeat::Grid {
                    clock,
                    every,
                    offset,
                    repeat,
                },
            };
            (None, Some(beat))
        }
        EditTime::AfterSamples(_) | EditTime::Condition(_) => (None, None),
    }
}

/// Adopts a landed timed edit on the reclaimer thread's cadence, so its
/// document change is recorded within one reclaim interval of it landing,
/// whether or not anything else takes the publisher. Holds the live graph
/// weakly, so the thread never keeps it alive.
pub(super) struct Adopter {
    /// Whether a timed edit waits: checked first, so a tick costs one
    /// atomic load while none does.
    waiting: Arc<AtomicBool>,
    publisher: Weak<Mutex<Publisher>>,
    surfaces: Weak<Mutex<IndexMap<String, ControlSurfaceInstance>>>,
    ports: Weak<Mutex<IndexMap<String, ModulePorts>>>,
    state: Weak<Mutex<RuntimeState>>,
}

impl Adopter {
    pub(super) fn tick(&self) {
        if !self.waiting.load(Ordering::Relaxed) {
            return;
        }
        let Some(publisher) = self.publisher.upgrade() else {
            return;
        };
        // Never waits: whoever holds the publisher may adopt it, and the
        // next tick will if not.
        let Ok(mut publisher) = publisher.try_lock() else {
            return;
        };
        if !publisher.timed_waiting() {
            return;
        }
        let surfaces = self.surfaces.upgrade();
        let ports = self.ports.upgrade();
        let state = self.state.upgrade();
        if let (Some(surfaces), Some(ports), Some(state)) = (surfaces, ports, state) {
            adopt_into(&mut publisher, (&surfaces, &ports, &state));
        }
    }
}
