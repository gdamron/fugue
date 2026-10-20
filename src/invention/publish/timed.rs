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

use std::sync::MutexGuard;

use super::change::BuiltModule;
use super::watch::{condition_of, Part};
use super::{publisher, LiveGraph, PreparedChange, Publisher, Refused};
use crate::control_request::{BeatSpec, BeatTime, RequestId, When};
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
        let Some(adopted) = publisher.adopt() else {
            return;
        };
        let publisher::Adopted {
            published,
            retain,
            port,
        } = adopted;
        self.commit_published(publisher, published, &port, retain);
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
        prepared: PreparedChange,
        time: EditTime<'_>,
        ttl: Option<u64>,
        retain: impl FnOnce(&mut RuntimeState) + Send + 'static,
    ) -> Result<RequestId, GraphCommandError> {
        self.reclaim();
        let mut publisher = self.lock();
        let result = self.resolve_edit_time(&publisher, &prepared, time);
        let when = match result {
            Ok(when) => when,
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
        match publisher.publish_timed(prepared, when, ttl, (Box::new(retain), port)) {
            Ok(id) => {
                pending.timed(id);
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
