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

// Until the live graph queues and adopts timed edits (FUG-323 4/5).
#![allow(dead_code)]

use std::sync::atomic::Ordering;

use super::{Published, Publisher, Refused};
use crate::control_request::{QueueFull, Request, RequestId, RequestValue, When};
use crate::invention::declared::RequestPort;
use crate::invention::graph::{Fate, TIMED_EDIT_MODULE};
use crate::invention::publish::change::{BuiltModule, PreparedChange, TopologyMirror};
use crate::invention::runtime::GraphCommandError;
use crate::invention::state::RuntimeState;
use crate::payload::Payload;
use indexmap::IndexMap;

/// Runs on the runtime state as a timed edit is adopted: records what it
/// changed in the document, as it applies.
pub(crate) type Retain = Box<dyn FnOnce(&mut RuntimeState) + Send>;

/// A timed edit queued and not yet adopted.
pub(crate) struct Waiting {
    fate: Fate,
    /// The topology once it lands.
    mirror: TopologyMirror,
    built: IndexMap<String, BuiltModule>,
    retain: Retain,
    /// Where its built modules' surfaces submit once adopted.
    port: RequestPort,
}

/// A timed edit the audio thread landed, adopted by the publisher: what it
/// replaced and built, for the runtime's other mirrors.
pub(crate) struct Adopted {
    pub(crate) published: Published,
    pub(crate) retain: Retain,
    pub(crate) port: RequestPort,
}

impl Publisher {
    /// Queues `prepared` to land on the audio thread `when` (any `when`
    /// but `Now`), refused after `ttl` samples, and returns the request's
    /// id. Refused with nothing queued as [`Self::publish`] refuses a
    /// change, when the change edits nothing, or while another timed edit
    /// waits (`ControlError`).
    #[allow(clippy::result_large_err)]
    #[cfg_attr(not(test), allow(dead_code))] // until the runtime calls `commit_when`
    pub(crate) fn publish_timed(
        &mut self,
        mut prepared: PreparedChange,
        when: When,
        ttl: Option<u64>,
        (retain, port): (Retain, RequestPort),
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
        let Some(mut publication) = prepared.publication.take() else {
            return refuse(control("Only a structural edit can be timed"), prepared);
        };
        publication.map_survivors(self.mirror.modules.keys().map(String::as_str));
        publication.generation = self.generation + 1;
        let fate = Fate::default();
        publication.fate = Some(fate.guard());
        let mut edit = Request::edit(self.generation, Payload::owned(publication));
        edit.target.module_idx = TIMED_EDIT_MODULE;
        edit.when = when;
        edit.ttl = ttl;
        let id = match self.requests.submit(edit) {
            Ok(id) => id,
            Err(QueueFull(edit)) => {
                let RequestValue::Edit(payload) = edit.value else {
                    unreachable!("the queue hands back the request it was given");
                };
                prepared.publication = payload.take_owned().ok();
                return refuse(GraphCommandError::QueueFull, prepared);
            }
        };
        self.timed = Some(Waiting {
            fate,
            mirror: prepared.mirror,
            built: prepared.built,
            retain,
            port,
        });
        Ok(id)
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
                self.timed = None;
                return None;
            }
            Some(true) => {}
        }
        let waiting = self.timed.take()?;
        self.generation += 1;
        // Releases every request and write queued before it to the audio
        // thread, which then knows nothing is tagged older (see `landing`).
        self.published.store(self.generation, Ordering::Release);
        let previous = std::mem::replace(&mut self.mirror, waiting.mirror);
        Some(Adopted {
            published: Published {
                previous,
                built: waiting.built,
            },
            retain: waiting.retain,
            port: waiting.port,
        })
    }

    /// Clears the way for a change publishing now: cancels a timed edit
    /// still waiting. False when it has landed and is not yet adopted:
    /// the change was prepared against a generation that has moved.
    pub(super) fn cancel_timed(&mut self) -> bool {
        let Some(waiting) = &self.timed else {
            return true;
        };
        if !waiting.fate.cancel() && waiting.fate.landed() {
            return false;
        }
        self.timed = None;
        true
    }
}
