//! Recompiling a live graph's process order when a schedule write changes
//! what its scheduler writes (FUG-388).
//!
//! A scheduler computes ahead of the modules whose controls it writes (see
//! `Module::control_targets`), and joins a feedback group with one that
//! drives it, but only as the topology was compiled. So a live `schedule`
//! write whose targets differ from those the installed topology was
//! compiled with publishes that topology recompiled first: the same
//! modules and routes, in the order a document loading the new schedule
//! would run them. The write is then submitted behind it, in the same
//! queue, so the audio thread never plays the new schedule in the old
//! order. A write leaving every module's targets as compiled publishes
//! nothing.
//!
//! The recompile is skipped while a timed edit waits, since publishing
//! would cancel it, and while edits back up with no reclaimer thread, as
//! an order is (see `watch`); the targets are then compiled at the next
//! change.

use std::any::Any;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use super::change::SurfaceMap;
use super::{PendingLog, PreparedChange, Publisher, Reclaimer, Refused};
use crate::control_request::RequestSender;

/// What a declared surface's live writes recompile the topology with.
#[derive(Clone)]
pub(crate) struct Retarget {
    /// The live graph's control-surface directory, weak so a surface never
    /// keeps the directory holding it alive.
    directory: Weak<Mutex<SurfaceMap>>,
    reclaimer: Arc<Reclaimer>,
    /// The queue surfaces submit to, refusing a request unless the room
    /// for one more edit stays free after it: whether a write still fits
    /// once its recompile has taken a slot.
    after_edit: RequestSender,
}

impl Retarget {
    pub(super) fn new(
        directory: &Arc<Mutex<SurfaceMap>>,
        reclaimer: &Arc<Reclaimer>,
        after_edit: RequestSender,
    ) -> Self {
        Self {
            directory: Arc::downgrade(directory),
            reclaimer: Arc::clone(reclaimer),
            after_edit,
        }
    }

    /// Under `publisher`: the topology recompiled with module `id` writing
    /// `targets`, or `None` when it needs none or cannot publish one now
    /// (see the module docs). Refuses, with nothing changed, when the write
    /// would not fit in the queue behind the recompile.
    pub(crate) fn prepare(
        &self,
        publisher: &Publisher,
        id: &str,
        targets: Vec<String>,
    ) -> Result<Option<PreparedChange>, String> {
        let Some(directory) = self.directory.upgrade() else {
            return Ok(None);
        };
        if publisher.timed_waiting() || self.backlogged(publisher) {
            return Ok(None);
        }
        let mut change = super::change_against(publisher, directory, &self.after_edit);
        if !change.retarget(id, targets) {
            return Ok(None);
        }
        if !self.has_room() {
            return Err("The control request queue is full; try again".into());
        }
        Ok(Some(change.compile()))
    }

    /// Whether a write would still fit in the queue once a recompile ahead
    /// of it has taken a slot.
    pub(crate) fn has_room(&self) -> bool {
        self.after_edit.has_room()
    }

    /// Publishes `prepared` under `publisher`. Returns what is left to free
    /// once the guard is released, or the refusal (nothing published).
    pub(crate) fn publish(
        &self,
        publisher: &mut Publisher,
        pending: &Mutex<PendingLog>,
        prepared: PreparedChange,
    ) -> Result<Box<dyn Any>, (String, Box<dyn Any>)> {
        // Under the publisher, as a commit records itself.
        pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .edited();
        match publisher.publish(prepared) {
            Ok(published) => {
                debug_assert!(published.built.is_empty(), "a recompile builds nothing");
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
