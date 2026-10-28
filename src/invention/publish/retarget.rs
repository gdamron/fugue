//! Recompiling a live graph's process order when a schedule write changes
//! what its scheduler writes (FUG-388).
//!
//! A scheduler computes ahead of the modules whose controls it writes (see
//! `Module::control_targets`), and joins a feedback group with one that
//! drives it, but only as the topology was compiled. So a live `schedule`
//! write whose targets differ from those the installed topology was
//! compiled with publishes that topology recompiled: the same modules and
//! routes, in the order a document loading the new schedule would run
//! them. The schedule travels in that publication, handed over with the
//! requests due as it installs (see `Handover::with_requests`), not as a
//! request beside it, which the audio thread could drain a block apart:
//! the new schedule never plays in the old order, nor the old one in the
//! new, and an entry on the install's sample plays as a request's would.
//! Any other scheduler whose last schedule write it has not adopted yet
//! (one refused for want of room, or still queued) is handed that schedule
//! as well, since the recompile orders it by that schedule's targets. A
//! write leaving every module's targets as compiled publishes nothing and
//! goes as a request.
//!
//! The recompile is skipped while a timed edit waits, since publishing
//! would cancel it, and while edits back up with no reclaimer thread, as
//! an order is (see `watch`); the targets are then compiled at the next
//! change.

use std::any::Any;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use super::change::SurfaceMap;
use super::{PendingLog, PreparedChange, Publisher, Reclaimer, Refused};
use crate::control_request::{ControlIndex, RequestSender};
use crate::invention::graph::Handover;
use crate::modules::control_scheduler::{Rebind, Written};
use crate::modules::ControlSchedulerControls;
use crate::payload::Payload;

/// A recompile ready to publish (see [`Retarget::prepare`]).
pub(crate) struct Recompile {
    prepared: PreparedChange,
    /// The schedules it hands other schedulers, to record once published.
    written: Vec<Written>,
}

/// What a declared surface's live writes recompile the topology with.
#[derive(Clone)]
pub(crate) struct Retarget {
    /// The live graph's control-surface directory, weak so a surface never
    /// keeps the directory holding it alive.
    directory: Weak<Mutex<SurfaceMap>>,
    reclaimer: Arc<Reclaimer>,
    requests: RequestSender,
}

impl Retarget {
    pub(super) fn new(
        directory: &Arc<Mutex<SurfaceMap>>,
        reclaimer: &Arc<Reclaimer>,
        requests: &RequestSender,
    ) -> Self {
        Self {
            directory: Arc::downgrade(directory),
            reclaimer: Arc::clone(reclaimer),
            requests: requests.clone(),
        }
    }

    /// Under `publisher`: the topology recompiled with module `id` writing
    /// `targets` and a schedule running `ramps` ramps at once, or `None`
    /// when it needs none (the targets compiled, and room enough for the
    /// tables' ramps) or cannot publish one now (see the module docs), or
    /// another scheduler's schedule no longer resolves.
    pub(crate) fn prepare(
        &self,
        publisher: &Publisher,
        id: &str,
        (targets, ramps): (Vec<String>, usize),
    ) -> Option<Recompile> {
        let directory = self.directory.upgrade()?;
        if publisher.timed_waiting() || self.backlogged(publisher) {
            return None;
        }
        let mut change = super::change_against(publisher, directory, &self.requests);
        // Both: a recompile for either takes the room the other needs.
        let grows = change.write_ramps(id, ramps);
        if !(change.retarget(id, targets) || grows) {
            return None;
        }
        let mut prepared = change.compile();
        let written = hand_unsettled(&mut prepared, id)?;
        Some(Recompile { prepared, written })
    }

    /// Publishes `recompile`, from [`Self::prepare`] for module `id`, under
    /// `publisher`, handing `payload` to the module's `control` as it
    /// installs. Returns what is left to free once the guard is released,
    /// or the refusal, with nothing published.
    pub(crate) fn publish(
        &self,
        publisher: &mut Publisher,
        pending: &Mutex<PendingLog>,
        (recompile, id): (Recompile, &str),
        (control, payload): (ControlIndex, Payload),
    ) -> Result<Box<dyn Any>, (String, Box<dyn Any>)> {
        let Recompile {
            mut prepared,
            written,
        } = recompile;
        // A survivor, so the mirror has it, in the publication's order.
        let module_idx = prepared.mirror.modules.get_index_of(id);
        match (prepared.publication.as_mut(), module_idx) {
            (Some(publication), Some(module_idx)) => {
                let handover = Handover::with_requests(module_idx, control, payload);
                publication.handovers.push(handover);
            }
            _ => {
                let refused = "The graph changed while this write was prepared; try again";
                return Err((refused.into(), Box::new((prepared, payload))));
            }
        }
        // Under the publisher, as a commit records itself; the write
        // reaches the graph without a request.
        let log = || pending.lock().unwrap_or_else(PoisonError::into_inner);
        log().edited();
        match publisher.publish(prepared) {
            Ok(published) => {
                debug_assert!(published.built.is_empty(), "a recompile builds nothing");
                log().unlogged();
                for schedule in written {
                    schedule.record();
                }
                Ok(Box::new(published))
            }
            Err(Refused { error, change }) => Err((error.to_string(), Box::new(change))),
        }
    }

    /// Whether one more edit could stall with no reclaimer thread (see
    /// `LiveGraph::backlogged`).
    fn backlogged(&self, publisher: &Publisher) -> bool {
        !self.reclaimer.threaded()
            && publisher.audio_alive()
            && super::backlogged(publisher, &self.reclaimer)
    }
}

/// Hands every scheduler but `id` whose last schedule write it has not
/// adopted that schedule as `prepared` installs, as a change rebinding it
/// does (see `ControlSchedulerControls::rebind`). Returns the schedules to
/// record, or `None` when one no longer resolves.
fn hand_unsettled(prepared: &mut PreparedChange, id: &str) -> Option<Vec<Written>> {
    let publication = prepared.publication.as_mut()?;
    let mut written = Vec::new();
    for (other, surface) in &prepared.surfaces {
        let any = surface.as_any();
        let controls = any.and_then(|any| any.downcast_ref::<ControlSchedulerControls>());
        let Some(controls) = controls.filter(|_| other != id) else {
            continue;
        };
        let Some(rebind) = controls
            .rebind(None, &HashSet::new(), &prepared.surfaces)
            .ok()?
        else {
            continue;
        };
        let module_idx = prepared.mirror.modules.get_index_of(&rebind.id)?;
        let handover = Handover::with_requests(module_idx, Rebind::CONTROL, rebind.payload);
        publication.handovers.push(handover);
        written.push(rebind.written);
    }
    Some(written)
}
