//! The control writes a timed edit makes to modules it keeps (a batch's
//! writes, a reload's config changes), chained to its landing: each is a
//! request waiting on the edit applying, so it plays from the edit's
//! sample and never before, and is refused with it (`ChainBroken`) when
//! the edit is cancelled, refused or expires.
//!
//! An authored write to one of those controls while the edit waits is kept
//! over it ([`Publisher::keep_authored`]): chained to the landing too, at
//! the same rank and queued after the edit's own write, so it wins there
//! and the author's value plays on. It replaces that write, and any copy
//! before it, as it reaches the audio thread, so the edit's own write to
//! that one control never sounds when the copy reaches it before the
//! landing; held back past it, the copy plays as soon as it does.

use super::{LiveGraph, PendingLog, PendingWrite, Publisher};
use crate::control_request::{
    Condition, Predicate, Request, RequestId, RequestValue, RtValue, When, CONDITION_CAPACITY,
};
use crate::invention::declared::DeclaredSurface;
use crate::invention::runtime::GraphCommandError;
use crate::ControlValue;

/// A survivor's write, resolved and ready to chain to an edit.
pub(super) struct Chained {
    module_id: String,
    key: String,
    request: Request,
    raw: RtValue,
    shown: Option<(String, ControlValue)>,
}

impl LiveGraph {
    /// Resolves `writes` (module, control, value) under `publisher`, before
    /// the edit is queued. Refused, so that nothing is queued, when a
    /// module keeps its controls outside the request queue (a legacy
    /// control surface), a value does not fit its control, or fewer
    /// requests can wait on conditions than there are writes.
    pub(super) fn resolve_chain(
        &self,
        publisher: &Publisher,
        writes: &[(String, String, ControlValue)],
    ) -> Result<Vec<Chained>, GraphCommandError> {
        let control = GraphCommandError::ControlError;
        if writes.is_empty() {
            return Ok(Vec::new());
        }
        let free = CONDITION_CAPACITY.saturating_sub(self.requests.watch_slots().taken());
        if writes.len() > free {
            return Err(control(format!(
                "A timed edit makes its {} control writes to modules it keeps as it lands, \
                 and only {free} more requests can wait now: try again once some apply",
                writes.len()
            )));
        }
        let surfaces = self.control_surfaces.lock().unwrap();
        let resolve = |(module_id, key, value): &(String, String, ControlValue)| {
            let surface = surfaces.get(module_id);
            let surface =
                surface.ok_or_else(|| GraphCommandError::UnknownModule(module_id.into()))?;
            let declared = surface
                .as_any()
                .and_then(|any| any.downcast_ref::<DeclaredSurface>())
                .ok_or_else(|| {
                    control(format!(
                        "A timed edit cannot write {module_id}.{key} as it lands: {module_id}'s \
                         controls cannot be timed yet. Write it untimed, or rebuild {module_id} \
                         in the edit"
                    ))
                })?;
            let write = declared
                .resolve_write(key, value)
                .map_err(|error| control(format!("{module_id}.{key}: {error}")))?;
            let target = publisher.control_target(module_id, write.index)?;
            let mut request = Request::new(target, RequestValue::Value(write.raw));
            request.event = write.event;
            Ok(Chained {
                module_id: module_id.clone(),
                key: key.clone(),
                request,
                raw: write.raw,
                shown: write.shown,
            })
        };
        writes.iter().map(resolve).collect()
    }

    /// Submits `chain`, each waiting on edit `edit` applying, before the
    /// edit itself is queued (see [`Publisher::publish_timed`]), so the
    /// audio thread has every write waiting when the edit lands; reads list
    /// each pending meanwhile. Refused with nothing queued (`QueueFull`)
    /// when the queue has no room for them all and the edit after them.
    /// Returns what each write is to, for [`Publisher::chain_timed`], and
    /// whether one was queued short of the chain: never while submissions
    /// are serialized, as they are under the publisher, but if so the edit
    /// is to be cancelled, so the writes queued are refused with it.
    pub(super) fn submit_chain(
        &self,
        pending: &mut PendingLog,
        edit: RequestId,
        chain: Vec<Chained>,
    ) -> Result<(Vec<(String, String)>, bool), GraphCommandError> {
        // The writes' sender leaves a slot past them for the edit's.
        if !self.requests.has_room_for(chain.len() as u64) {
            return Err(GraphCommandError::QueueFull);
        }
        let condition = Condition::all(&[Predicate::Applied(edit)]).expect("one part");
        let mut chained = Vec::with_capacity(chain.len());
        for write in chain {
            let Chained {
                module_id,
                key,
                mut request,
                raw,
                shown,
            } = write;
            request.when = When::Condition(condition);
            let (source, when) = (request.source, request.when);
            let Ok(id) = self.requests.submit(request) else {
                return Ok((chained, true));
            };
            if let Some((shown_key, value)) = shown {
                let write = PendingWrite {
                    module_id: module_id.clone(),
                    key: shown_key,
                    value,
                    raw: Some(raw),
                    beat: None,
                    source,
                    when,
                    authored: None,
                    announced: false,
                    awaited: false,
                };
                pending.submitted(id, write);
            }
            chained.push((module_id, key));
        }
        Ok((chained, false))
    }
}
