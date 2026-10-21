//! Recompiling a live graph's process order when a schedule write changes
//! what its scheduler writes (FUG-388).
//!
//! A scheduler computes ahead of the modules whose controls it writes (see
//! `Module::control_targets`), and joins a feedback group with one that
//! drives it, but only as the topology was compiled. So a live `schedule`
//! write whose targets differ from those the installed topology was
//! compiled with publishes that topology recompiled: the same modules and
//! routes, in the order a document loading the new schedule would run
//! them. The schedule travels in that publication, handed over as it
//! installs (see `graph::publication::handover`), not as a request behind
//! it, which the audio thread could drain a block later: the new schedule
//! never plays in the old order, nor the old one in the new. A write
//! leaving every module's targets as compiled publishes nothing and goes
//! as a request.
//!
//! The recompile is skipped while a timed edit waits, since publishing
//! would cancel it, and while edits back up with no reclaimer thread, as
//! an order is (see `watch`); the targets are then compiled at the next
//! change.

use std::any::Any;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use super::change::SurfaceMap;
use super::{PendingLog, PreparedChange, Publisher, Reclaimer, Refused};
use crate::control_request::{ControlIndex, RequestSender};
use crate::invention::graph::Handover;
use crate::payload::Payload;

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
    /// `targets`, or `None` when it needs none or cannot publish one now
    /// (see the module docs).
    pub(crate) fn prepare(
        &self,
        publisher: &Publisher,
        id: &str,
        targets: Vec<String>,
    ) -> Option<PreparedChange> {
        let directory = self.directory.upgrade()?;
        if publisher.timed_waiting() || self.backlogged(publisher) {
            return None;
        }
        let mut change = super::change_against(publisher, directory, &self.requests);
        change.retarget(id, targets).then(|| change.compile())
    }

    /// Publishes `prepared`, from [`Self::prepare`] for module `id`, under
    /// `publisher`, handing `payload` to the module's `control` as it
    /// installs. Returns what is left to free once the guard is released,
    /// or the refusal, with nothing published.
    pub(crate) fn publish(
        &self,
        publisher: &mut Publisher,
        pending: &Mutex<PendingLog>,
        (mut prepared, id): (PreparedChange, &str),
        (control, payload): (ControlIndex, Payload),
    ) -> Result<Box<dyn Any>, (String, Box<dyn Any>)> {
        // A survivor, so the mirror has it, in the publication's order.
        let module_idx = prepared.mirror.modules.get_index_of(id);
        match (prepared.publication.as_mut(), module_idx) {
            (Some(publication), Some(module_idx)) => {
                let handover = Handover::new(module_idx, control, payload);
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
