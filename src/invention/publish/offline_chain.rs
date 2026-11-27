//! The control writes a timed batch makes to modules it keeps, on an
//! offline render, chained to its landing as a live graph's are (see
//! `timed_chain`): each waits in the render's queue on the edit applying,
//! so it plays from the edit's sample and never before, and is refused
//! with it when the edit is cancelled, refused or expires. Each is
//! recorded in the document as it applies, after the edit's change.
//!
//! An authored write to one of those controls while the edit waits is kept
//! over it, as on a live graph. The write applies as it is made, as an
//! offline write does, and a copy waits on the edit landing, at the
//! write's rank and queued after the edit's own write, so the author's
//! value plays on there ([`keep_offline_authored`]). A later copy replaces
//! it, so a drag holds one request.

use crate::control_request::{
    Condition, ControlIndex, ControlTarget, Intent, Predicate, Request, RequestId, RequestValue,
    RtValue, Source, When, CONDITION_CAPACITY,
};
use crate::invention::declared::DeclaredSurface;
use crate::invention::graph::{OfflineWrite, SignalGraph};
use crate::invention::publish::timed_write::TIMED_WRITE_ROOM;
use crate::invention::runtime::GraphCommandError;
use crate::modules::control_scheduler::SurfaceDirectory;
use crate::ControlValue;

/// A write to a kept module, resolved and ready to chain to an edit.
pub(super) struct OfflineChained {
    request: Request,
    write: OfflineWrite,
}

/// Resolves `writes` (module, control, value) on an offline render's
/// `locked` graph, before the edit is queued. Refused with the live graph's
/// reasons (see `LiveGraph::resolve_chain`), so that nothing is queued, when
/// a module keeps its controls outside the request queue (an undeclared
/// control), a value does not fit its control, or fewer requests can wait
/// than there are writes.
pub(super) fn resolve_offline_chain(
    locked: &SignalGraph,
    surfaces: &SurfaceDirectory,
    writes: &[(String, String, ControlValue)],
) -> Result<Vec<OfflineChained>, GraphCommandError> {
    let control = GraphCommandError::ControlError;
    if writes.is_empty() {
        return Ok(Vec::new());
    }
    let offline = locked.offline.as_ref();
    let offline = offline.ok_or(GraphCommandError::AudioThreadStopped)?;
    // No more than an offline render's timed writes may wait at once, too.
    let free = CONDITION_CAPACITY.saturating_sub(offline.sender.watch_slots().taken());
    let free = free.min(TIMED_WRITE_ROOM.saturating_sub(offline.waiting().1));
    if writes.len() > free {
        return Err(control(format!(
            "A timed edit makes its {} control writes to modules it keeps as it lands, \
             and only {free} more requests can wait now: try again once some apply",
            writes.len()
        )));
    }
    let generation = locked.request_generation();
    let surfaces = surfaces.lock().unwrap();
    let resolve = |(module_id, key, value): &(String, String, ControlValue)| {
        let unknown = || GraphCommandError::UnknownModule(module_id.into());
        let surface = surfaces.get(module_id).ok_or_else(unknown)?;
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
        let module_idx = locked.modules.get_index_of(module_id.as_str());
        let target = ControlTarget {
            generation,
            module_idx: module_idx.ok_or_else(unknown)?,
            control: write.index,
        };
        let mut request = Request::new(target, RequestValue::Value(write.raw));
        request.event = write.event;
        let write = OfflineWrite {
            module_id: module_id.clone(),
            key: key.clone(),
            when: When::Now,
            beat: None,
            // As the edit names it, never an event.
            authored: (!write.event).then(|| value.clone()),
            generation,
            announced: false,
        };
        Ok(OfflineChained { request, write })
    };
    writes.iter().map(resolve).collect()
}

/// Submits `chain` on `locked`'s queue, each waiting on edit `edit`
/// applying, before the edit itself is queued, so the render has every
/// write waiting when it lands. Returns what each write is to. Never
/// fails, since the caller checked room for the chain and the edit under
/// the graph's lock; if it does (`QueueFull`), the edit is marked
/// cancelled, so the writes already queued are refused.
pub(super) fn submit_offline_chain(
    locked: &mut SignalGraph,
    edit: RequestId,
    chain: Vec<OfflineChained>,
) -> Result<Vec<(String, String)>, GraphCommandError> {
    let Some(offline) = locked.offline.as_mut() else {
        return Err(GraphCommandError::AudioThreadStopped);
    };
    let when = When::Condition(Condition::all(&[Predicate::Applied(edit)]).expect("one part"));
    let mut chained = Vec::with_capacity(chain.len());
    for OfflineChained {
        mut request,
        mut write,
    } in chain
    {
        request.when = when;
        write.when = when;
        let Ok(id) = offline.sender.submit(request) else {
            offline.sender.cancelled(edit);
            return Err(GraphCommandError::QueueFull);
        };
        chained.push((write.module_id.clone(), write.key.clone()));
        offline.submitted(id, write);
    }
    Ok(chained)
}

/// Keeps an authored write of `raw` to control `index` (`key`) of
/// `module_id`, which just applied on an offline render's `locked` graph,
/// over the timed edit waiting there when that edit writes the control as
/// it lands, as a live graph keeps one (see `Publisher::keep_authored`): a
/// copy, at the write's `(source, priority, ttl)`, waits on the edit
/// landing and replaces any copy before it. Returns the edit it is kept
/// over, or `None` with nothing queued when none waits, it does not write
/// the control, or no request can wait (a write then cancels the edit, as
/// it is reconciled). Each copy holds a slot until the render takes it, so
/// a host writing many times between two renders can run out of room, and
/// the write then cancels the edit, as on a live graph whose queue fills.
pub(crate) fn keep_offline_authored(
    locked: &mut SignalGraph,
    (module_id, key): (&str, &str),
    (index, raw): (ControlIndex, RtValue),
    (source, priority, ttl): (Source, i16, Option<u64>),
) -> Option<RequestId> {
    let target = ControlTarget {
        generation: locked.request_generation(),
        module_idx: locked.modules.get_index_of(module_id)?,
        control: index,
    };
    let offline = locked.offline.as_mut()?;
    offline.settle();
    let waiting = offline.edits.waiting()?;
    let chained = |(module, control): &(String, String)| module == module_id && control == key;
    if waiting.fate.settled().is_some() || !waiting.chained.iter().any(chained) {
        return None;
    }
    if offline.sender.watchers_full(false) || !offline.sender.has_room() {
        return None;
    }
    let edit = waiting.id;
    let mut copy = Request::new(target, RequestValue::Value(raw));
    copy.intent = Intent::Author;
    copy.source = source;
    copy.priority = priority;
    copy.ttl = ttl;
    copy.when = When::Condition(Condition::kept_applied(edit));
    offline.sender.submit(copy).ok()?;
    offline
        .edits
        .ledger
        .kept(edit, format!("{module_id}.{key}"));
    Some(edit)
}
