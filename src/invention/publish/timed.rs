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
use super::timed_status::{CancelCause, EditStatus, Touched};
use super::watch::{condition_of, Part};
use super::{
    publisher, Committed, LiveGraph, Mirrors, PendingLog, PreparedChange, Publisher, Refused,
};
use crate::control_request::{BeatSpec, BeatTime, RequestId, When};
use crate::invention::orchestration::ModulePorts;
use crate::invention::request_log::{LoggedBeat, LoggedEdit, LoggedWhen};
use crate::invention::runtime::ControlSurfaceInstance;
use crate::invention::runtime::GraphCommandError;
use crate::invention::state::RuntimeState;
use crate::ControlValue;
use crate::ModuleRegistry;

/// What became of an authored write as it was recorded (see
/// [`LiveGraph::record_authored`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Authored {
    /// Recorded: what plays.
    Recorded,
    /// Not recorded: a timed edit landed after the write and rebuilt or
    /// removed its module (or re-resolved its schedule), or wrote the
    /// control itself, so what plays may not be the write. It is to be
    /// made again.
    Overtaken,
}

/// When a timed edit lands, resolved against the graph that is playing.
#[derive(Clone, Copy, Debug)]
pub(crate) enum EditTime<'a> {
    /// On this sample of the transport.
    AtSample(u64),
    /// This many samples from now.
    AfterSamples(u64),
    /// On clock `clock`'s beat `spec` (one beat, not a repeating grid):
    /// the sample the clock reaches it on.
    Beat { clock: &'a str, spec: BeatSpec },
    /// [`Self::Beat`] on the clock module `module` counts on (see
    /// `crate::invention::timelines`).
    BeatOf { module: &'a str, spec: BeatSpec },
    /// [`Self::Beat`] on the invention's first clock.
    FirstClock(BeatSpec),
    /// Once every one of `parts` has happened: on the sample when that is
    /// a segment start (another request applying, a control crossing),
    /// otherwise at the next segment start, reported late. Not on the wire
    /// yet: a condition is FUG-317's to name.
    #[cfg_attr(not(test), allow(dead_code))]
    Condition(&'a [Part<'a>]),
}

/// Module types whose scripts or agents a commit starts and stops, which
/// a timed edit cannot do yet as it lands.
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
    pub(crate) fn timed_status(&self, id: RequestId) -> Option<EditStatus> {
        let mut publisher = self.lock();
        self.sync_timed(&mut publisher);
        publisher.ledger().status(id)
    }

    /// Every timed edit whose status changed since the last call, in the
    /// order they changed.
    pub(crate) fn take_timed_changes(&self) -> Vec<EditStatus> {
        let mut publisher = self.lock();
        self.sync_timed(&mut publisher);
        publisher.ledger().take_changes()
    }

    /// What adopting timed edit `id` committed (its built modules'
    /// handles), once it is adopted; taken once.
    pub(crate) fn take_timed_committed(&self, id: RequestId) -> Option<Committed> {
        self.lock().ledger().take_committed(id)
    }

    /// Takes the audio thread's outcomes for timed edits into the ledger.
    fn sync_timed(&self, publisher: &mut Publisher) {
        sync_ledger(publisher, &mut self.pending.lock().unwrap());
    }

    /// Records an authored write to control `key` of `module_id`, queued
    /// at generation `seen` and `safe` from the timed edit it names (see
    /// `declared::Queued`), in the retained document. Runs under the
    /// publisher, so no timed edit can be adopted in between.
    ///
    /// [`Authored::Overtaken`], with nothing recorded and nothing else
    /// done, when a timed edit adopted since `seen` may have landed after
    /// the write and replaced what it wrote (see
    /// [`Publisher::timed_overtook`]): the caller makes it again. One that
    /// touched neither the module, its schedule nor the control leaves the
    /// write playing, so it is recorded, as it is when only untimed changes
    /// were published since `seen` (they publish nothing after the write
    /// reaches the audio thread).
    ///
    /// Otherwise a timed edit waiting that rebuilds or removes the module
    /// is cancelled, and the edit reports why, as is one that writes the
    /// control as it lands but did not keep the write over its own (see
    /// [`Publisher::keep_authored`]): the write may reach the audio thread
    /// before or after the edit lands, so only that makes the write what
    /// plays, and what is recorded. One that keeps the module folds the
    /// write into its own document, so the write is recorded again as the
    /// edit is adopted.
    pub(crate) fn record_authored(
        &self,
        module_id: &str,
        key: &str,
        value: &ControlValue,
        (seen, safe): (u64, Option<RequestId>),
    ) -> Authored {
        let mut publisher = self.lock();
        if publisher.timed_overtook(seen, (module_id, key), safe) {
            return Authored::Overtaken;
        }
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
            // Landed meanwhile, after the write: adopted, it overtook it.
            if !publisher.cancel_timed(cause) {
                self.adopt(&mut publisher);
                return Authored::Overtaken;
            }
        }
        publisher.fold_timed(module_id, key, value);
        let mut state = self.state.lock().unwrap();
        state.record_authored_control(module_id, key, value);
        Authored::Recorded
    }

    /// Cancels the timed edit waiting, if any, as the invention stops, or
    /// adopts it when it has landed (see `RunningInvention::settle_requests`).
    pub(crate) fn settle_timed(&self) {
        let mut publisher = self.lock();
        if !publisher.cancel_timed(|| CancelCause::Stopping) {
            self.adopt(&mut publisher);
        }
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
    #[cfg_attr(not(test), allow(dead_code))] // the runtime builds through `commit_built_when`
    pub(crate) fn commit_when(
        &self,
        prepared: PreparedChange,
        time: EditTime<'_>,
        ttl: Option<u64>,
        retain: impl FnOnce(&mut RuntimeState) + Send + 'static,
    ) -> Result<RequestId, GraphCommandError> {
        self.commit_built_when(prepared, None, time, ttl, retain, &[])
    }

    /// [`Self::commit_when`] for a change whose modules were built against
    /// `built_with`: refused `TopologyMoved` when a commit has adopted
    /// another registry since, so no edit queues an instance of a
    /// superseded definition. `writes` (module, control, value) to modules
    /// it keeps play from the sample it lands on (see `timed_chain`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn commit_built_when(
        &self,
        mut prepared: PreparedChange,
        built_with: Option<&Arc<ModuleRegistry>>,
        time: EditTime<'_>,
        ttl: Option<u64>,
        retain: impl FnOnce(&mut RuntimeState) + Send + 'static,
        writes: &[(String, String, ControlValue)],
    ) -> Result<RequestId, GraphCommandError> {
        self.reclaim();
        let mut publisher = self.lock();
        let superseded = built_with.is_some_and(|registry| !self.registry.is_current(registry));
        if superseded || prepared.base_generation != publisher.generation() {
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
            .and_then(|when| Ok((when, self.resolve_chain(&publisher, writes)?)))
            .and_then(|resolved| Ok((resolved, rebind::pack(&mut prepared)?)));
        let (((when, beat), chain), packed) = match result {
            Ok(resolved) => resolved,
            Err(error) => {
                drop(publisher);
                drop(prepared);
                return Err(error);
            }
        };
        let logged = logged_edit(&publisher, &prepared, time, beat.as_ref());
        let port = self.port();
        // Recorded in the request log as it settles: registered under the
        // log, locked before the submission, as a write is (see `watch`).
        let mut pending = self.pending.lock().unwrap();
        pending.settle();
        let mut submitted = (Vec::new(), false);
        // The writes are queued ahead of the edit, so the audio thread has
        // them waiting when it lands, however soon.
        let before = |id| {
            pending.timed(id, logged);
            submitted = self.submit_chain(&mut pending, id, chain)?;
            Ok(())
        };
        let parts = (Box::new(retain) as _, port, packed);
        match publisher.publish_timed(prepared, when, ttl, parts, before) {
            Ok(id) => {
                let (chained, short) = submitted;
                publisher.chain_timed(chained);
                let sample = match time {
                    EditTime::AtSample(sample) => Some(sample),
                    _ => None,
                };
                publisher.ledger().queued(id, sample, beat);
                // Short of its chain only if a submission raced this one
                // outside the publisher, which none does: then the edit is
                // cancelled, refusing the writes queued with it. One that
                // landed already is adopted as it is, and reported as what
                // it is, never as refused.
                let unchained = short && publisher.cancel_timed(|| CancelCause::Unchained);
                drop(pending);
                drop(publisher);
                self.settle();
                if unchained {
                    return Err(GraphCommandError::QueueFull);
                }
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

    /// The request time `time` resolves to, with its beat for the ledger
    /// when it is timed in beats.
    fn resolve_edit_time(
        &self,
        publisher: &Publisher,
        prepared: &PreparedChange,
        time: EditTime<'_>,
    ) -> Result<(When, Option<LoggedBeat>), GraphCommandError> {
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
        let beat = |clock: u32, spec: BeatSpec| {
            let id = mirror.modules.get_index(clock as usize).map(|(id, _)| id);
            let when = When::Beat(BeatTime {
                clock,
                spec,
                ramp: None,
            });
            (when, id.map(|id| logged_beat(id, spec)))
        };
        Ok(match time {
            EditTime::AtSample(sample) => (When::AtSample(sample), None),
            EditTime::AfterSamples(samples) => (When::AfterSamples(samples), None),
            EditTime::Beat { clock, spec } => beat(publisher.beat_clock(clock, Some(clock))?, spec),
            EditTime::BeatOf { module, spec } => beat(publisher.beat_clock(module, None)?, spec),
            EditTime::FirstClock(spec) => beat(publisher.first_clock()?, spec),
            EditTime::Condition(parts) => {
                if self.requests.watchers_full() {
                    let full = "Too many requests wait on a condition; try again once some apply";
                    return Err(control(full.into()));
                }
                let surfaces = self.control_surfaces.lock().unwrap();
                (
                    When::Condition(condition_of(mirror, &surfaces, parts)?),
                    None,
                )
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
        folded,
    }) = publisher.adopt()
    else {
        return;
    };
    // Under the publisher, as a commit records them.
    for schedule in written {
        schedule.record();
    }
    let retain = move |state: &mut RuntimeState| {
        retain(state);
        // Written to the graph that was playing, to modules this edit
        // kept, after its document was prepared.
        for (module_id, key, value) in &folded {
            state.record_authored_control(module_id, key, value);
        }
    };
    let committed = LiveGraph::commit_mirrors(mirrors, publisher, published, &port, retain);
    publisher.ledger().adopted(id, committed);
}

/// Takes `pending`'s outcomes for timed edits into `publisher`'s ledger.
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

/// What the request log records of `prepared`, queued under `publisher`
/// at `time` (`beat`, resolved): none when it cannot replay it.
fn logged_edit(
    publisher: &Publisher,
    prepared: &PreparedChange,
    time: EditTime<'_>,
    beat: Option<&LoggedBeat>,
) -> Option<LoggedEdit> {
    let when = match (time, beat) {
        (EditTime::AtSample(sample), _) => LoggedWhen::AtSample { sample },
        (EditTime::AfterSamples(samples), _) => LoggedWhen::AfterSamples { samples },
        (EditTime::Condition(_), _) => LoggedWhen::Condition,
        (_, Some(beat)) => LoggedWhen::Beat { beat: beat.clone() },
        (_, None) => return None,
    };
    let after = (&prepared.mirror.modules, &prepared.mirror.edges[..]);
    LoggedEdit::queued(when, &publisher.mirror().modules, after, &prepared.built)
}

/// `spec` on clock `clock`, as the request log and the ledger show it.
fn logged_beat(clock: &str, spec: BeatSpec) -> LoggedBeat {
    let clock = clock.to_string();
    match spec {
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
