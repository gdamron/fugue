//! Delivering a write to a declared control by the route its surface takes.

use super::options::{Queued, Write, WriteOptions, STALE};
use super::{DeclaredSurface, Route, Staged};
use crate::control_request::{
    ControlIndex, ControlTable, DeclKind, Intent, Rank, Refusal, Request, RequestValue, RtValue,
    When,
};
use crate::invention::publish::{kept_until, timed_write, Authoring, CancelCause, PendingWrite};

const QUEUE_FULL: &str = "The control request queue is full; try again";

/// Why a timed write is refused where it cannot wait for its time.
const UNTIMED: &str = "A timed control write needs a running graph and a declared control";
const SCHEDULE_UNTIMED: &str =
    "A schedule write cannot be timed yet: time an edit batch that writes it instead";
const PAYLOAD_UNCHAINED: &str = "A payload write cannot wait on another request yet";
const CHAINS_FULL: &str = "Too many requests wait on another; try again once some apply";

#[cfg_attr(not(test), allow(dead_code))]
impl DeclaredSurface {
    /// `authored` names the control an authored write is to, as written.
    pub(super) fn deliver(
        &self,
        index: ControlIndex,
        write: Write,
        options: &WriteOptions,
        authored: Option<&str>,
    ) -> Result<Queued, String> {
        let (event, reserved, clamp, whole) = match self.table.decl(index) {
            Some((decl, _)) => {
                // A development's number clamps only to a range it
                // declares, which it holds writes within itself.
                let clamp = (decl.clamp)
                    .filter(|_| self.clamps || matches!(decl.kind, DeclKind::Number { .. }));
                (decl.event, decl.reserved, clamp, decl.whole)
            }
            None => (false, false, None, false),
        };
        let timed = options.time.is_some();
        let route = {
            let mut route = self.route.lock().unwrap();
            if let Route::Building(written) = &mut *route {
                if timed {
                    return Err(UNTIMED.into());
                }
                // Under the route lock, so a bind sees it or it sees the bind.
                let value = match write {
                    Write::Value(value) => value,
                    Write::Payload(payload, shown, checked) => {
                        self.show(index, shown, Some(payload));
                        if let Some(checked) = checked {
                            (checked.record)();
                        }
                        restage(written, index);
                        return Ok(Queued::default());
                    }
                };
                // Held as the module will hold it once it applies it.
                let held = match (value, clamp) {
                    (RtValue::F32(number), _) if whole => {
                        let (min, max) = clamp.unwrap_or((f32::MIN, f32::MAX));
                        RtValue::F32(number.trunc().max(min).min(max))
                    }
                    (RtValue::F32(number), Some((min, max))) => {
                        RtValue::F32(number.max(min).min(max))
                    }
                    (RtValue::I32(whole), Some((min, max))) => {
                        RtValue::I32(whole.clamp(min as i32, max as i32))
                    }
                    _ => value,
                };
                if event && self.cells.queues_events() {
                    written.push(Staged::Event(index, held));
                    return Ok(Queued::default());
                }
                self.cells.publish(index, held);
                restage(written, index);
                return Ok(Queued::default());
            }
            route.clone()
        };
        if timed && !matches!(route, Route::Live(_)) {
            return Err(UNTIMED.into());
        }
        match route {
            Route::Building(_) => unreachable!("handled under the lock"),
            Route::Live(port) => {
                // Off the publisher's lock, since what it frees must not
                // drop under it.
                port.reclaimer.reclaim_unthreaded();
                let publisher = port
                    .publisher
                    .upgrade()
                    .ok_or("The audio thread has stopped")?;
                let mut publisher = publisher.lock().unwrap();
                self.check_still(|route| matches!(route, Route::Live(_)))?;
                // A kept controller can outlive the audio graph: never queue
                // a request nothing will drain.
                if !publisher.audio_alive() {
                    return Err("The audio thread has stopped".into());
                }
                let target = publisher
                    .control_target(&port.module_id, index)
                    .map_err(|error| error.to_string())?;
                let (value, shown, mut checked) = match write {
                    Write::Value(value) => (RequestValue::Value(value), None, None),
                    Write::Payload(payload, shown, checked) => {
                        (RequestValue::Payload(payload), Some(shown), checked)
                    }
                };
                if checked
                    .as_ref()
                    .is_some_and(|checked| checked.generation != Some(target.generation))
                {
                    return Err(STALE.into());
                }
                if timed && checked.is_some() {
                    return Err(SCHEDULE_UNTIMED.into());
                }
                if let Some(checked) = &checked {
                    if publisher.timed_races(&port.module_id, &checked.targets) {
                        // Sure of its room first, so a full queue never
                        // cancels the edit, as a change publishing is.
                        if !port.requests.has_room() {
                            return Err(QUEUE_FULL.into());
                        }
                        let cause = || CancelCause::ScheduleWrite {
                            module_id: port.module_id.clone(),
                        };
                        if !publisher.cancel_timed(cause) {
                            return Err(STALE.into());
                        }
                    }
                }
                // A write retargeting its module travels in the topology
                // recompiled for it (see `publish::retarget`).
                let recompile = match &mut checked {
                    Some(checked) => {
                        let targets = std::mem::take(&mut checked.targets);
                        let written = (targets, checked.ramps);
                        port.retarget.prepare(&publisher, &port.module_id, written)
                    }
                    None => None,
                };
                let value = match (recompile, value) {
                    // A schedule that changes its targets travels in the
                    // recompile's publication, not as a request: it lands
                    // whatever its rank or ttl, as the last write made, and
                    // reads back as what plays.
                    (Some(prepared), RequestValue::Payload(payload)) => {
                        let change = (prepared, port.module_id.as_str());
                        let handed = (index, payload);
                        let published =
                            port.retarget
                                .publish(&mut publisher, &port.pending, change, handed);
                        let published = match published {
                            Ok(published) => published,
                            Err((error, refused)) => {
                                drop(publisher);
                                drop(refused);
                                return Err(error);
                            }
                        };
                        if let Some(shown) = shown {
                            self.show(index, shown, None);
                        }
                        if let Some(checked) = checked {
                            (checked.record)();
                        }
                        let generation = publisher.generation();
                        // What the recompile replaced is freed off the lock.
                        drop(publisher);
                        drop(published);
                        if let Some(settler) = &port.settler {
                            settler.settle();
                        }
                        // No timed edit waits (`Retarget::prepare` publishes
                        // none while one does), so none can overtake it, and
                        // a payload is never kept.
                        return Ok(Queued {
                            published: Some(generation),
                            ..Queued::default()
                        });
                    }
                    (_, value) => value,
                };
                // A payload is listed pending by its read-back, which the
                // session log records once it applies (see
                // `PendingWrite::raw`).
                let (read, raw) = match (&value, &shown) {
                    (RequestValue::Value(value), _) => {
                        (self.table.value(index, *value), Some(*value))
                    }
                    (_, shown) => (shown.clone(), None),
                };
                let mut request = Request::new(target, value);
                request.event = event;
                request.reserved = reserved;
                request.intent = match options.authored {
                    Some(_) => Intent::Author,
                    None => Intent::Perform,
                };
                request.source = options.source;
                request.priority = options.priority;
                request.ttl = options.ttl;
                // Never kept on a graph with no clock: there the copy's
                // hold, at a sample that never passes, would refuse every
                // later write to the control. The edit is cancelled instead.
                let clocked = port.settler.is_none();
                // The log is locked before anything is submitted, the copy
                // included, so neither's outcome can be received before
                // the write is recorded.
                let mut pending = port.pending.lock().unwrap();
                pending.settle();
                let beat = match options.time {
                    Some(time) => {
                        let conditions = match &request.value {
                            RequestValue::Payload(_) => Err(PAYLOAD_UNCHAINED),
                            _ if port.requests.watchers_full(reserved) => Err(CHAINS_FULL),
                            _ => Ok(()),
                        };
                        let module = port.module_id.as_str();
                        let locked = (&mut *publisher, &mut *pending);
                        let resolved =
                            timed_write::resolve(time, module, locked, (conditions, reserved))?;
                        // Counted from now as the sender would count it,
                        // so the log knows the sample it is due on.
                        request.when = match resolved.when {
                            When::AfterSamples(samples) => {
                                When::AtSample(port.requests.rendered().saturating_add(samples))
                            }
                            when => when,
                        };
                        resolved.beat
                    }
                    None => None,
                };
                let (source, when, ttl) = (request.source, request.when, request.ttl);
                // A timed write is never kept: the later of it and the
                // edit plays (see `publish::timed_write`).
                let kept = match (&request.value, authored) {
                    _ if timed => None,
                    (RequestValue::Value(raw), Some(key)) if clocked => {
                        let module = (port.module_id.as_str(), key);
                        let write = (*raw, event, options);
                        publisher.keep_authored(module, target, write, &port.requests)
                    }
                    // Any other authored write (a payload, one on a graph
                    // with no clock) queued after an edit landed follows its
                    // writes too, as one kept would.
                    _ if options.authored.is_some() => {
                        publisher.timed_landed().map(|id| (id, None))
                    }
                    _ => None,
                };
                let (safe, kept) = (kept.map(|(safe, _)| safe), kept.and_then(|(_, kept)| kept));
                let submitted = match kept {
                    Some(kept) => port.requests.submit_as(kept.write, request),
                    None => port.requests.submit(request),
                };
                let id = match submitted {
                    Ok(id) => id,
                    // Never, as `keep_authored` kept it only with room for
                    // this write too. A write not made is not to play: its
                    // copy, waiting on it, is refused at the drain's next
                    // segment start, and the edit is left alone.
                    Err(_) => {
                        if let Some(kept) = kept {
                            port.requests.cancelled(kept.write);
                        }
                        return Err(QUEUE_FULL.into());
                    }
                };
                // Under the publisher's lock, so reads report the last write
                // submitted.
                if let Some(shown) = shown {
                    self.show(index, shown, None);
                }
                if let (Some(key), Some(written)) = (self.table.key(index), read) {
                    let module_id = port.module_id.clone();
                    let write = PendingWrite {
                        module_id,
                        key,
                        value: written,
                        raw,
                        beat,
                        source,
                        when,
                        expires: port.requests.expires_by(ttl),
                        announced: options.announced,
                        awaited: options.awaited,
                        // Safe from the timed edit it was kept over, or that
                        // landed before it: its record follows that edit's.
                        authored: options.authored.map(|authoring| Authoring {
                            safe,
                            ..authoring.clone()
                        }),
                    };
                    // The copy applying, too, is this write's, though only the
                    // write is announced: the copy is the edit's.
                    if let Some(kept) = kept {
                        let copy_write = PendingWrite {
                            when: safe.map_or(when, |edit| kept_until(edit, id)),
                            announced: false,
                            awaited: false,
                            ..write.clone()
                        };
                        pending.submitted(kept.copy, copy_write);
                    }
                    pending.submitted(id, write);
                }
                drop(pending);
                if let Some(checked) = checked {
                    (checked.record)();
                }
                // Settled with no lock held: other writers carry on, and a
                // request one of them queues meanwhile applies in the same
                // block or theirs.
                drop(publisher);
                if let Some(settler) = &port.settler {
                    settler.settle();
                }
                Ok(Queued {
                    safe,
                    id: Some(id),
                    ..Queued::default()
                })
            }
            Route::Offline { graph, module_id } => {
                let rank = Rank::new(options.source, options.priority);
                let graph = graph.upgrade().ok_or("The render has been replaced")?;
                let mut graph = graph.lock().unwrap();
                self.check_still(|route| matches!(route, Route::Offline { .. }))?;
                match write {
                    Write::Value(value) => graph.apply_control(&module_id, index, value, rank),
                    Write::Payload(payload, shown, checked) => {
                        // Resolved before an offline edit installed: it may
                        // name what the edit replaced (see `Checked`).
                        let generation = checked.as_ref().and_then(|c| c.generation);
                        if generation.is_some_and(|seen| seen != graph.offline_edits) {
                            return Err(STALE.into());
                        }
                        let standing = (rank, options.due);
                        let applied = graph.apply_payload(&module_id, index, payload, standing);
                        if applied == Ok(true) {
                            self.show(index, shown, None);
                            if let Some(checked) = checked {
                                (checked.record)();
                                graph.recompile_if_retargeted(&module_id);
                                // On this thread, under the render's lock.
                                graph.fit_table_ramps();
                            }
                        }
                        applied
                    }
                }
                .map(|applied| Queued {
                    lost: !applied,
                    ..Queued::default()
                })
                .map_err(|refusal| refused(&self.table, index, refusal))
            }
            Route::Inner => Err("This control is set through its development".into()),
            Route::Prepared => Err("This module is being installed; try again".into()),
            Route::Retired => Err("This module has been removed or replaced".into()),
        }
    }

    /// Refuses a write whose module was retired after the route was read.
    fn check_still(&self, live: impl Fn(&Route) -> bool) -> Result<(), String> {
        if live(&self.route.lock().unwrap()) {
            Ok(())
        } else {
            Err("This module has been removed or replaced".into())
        }
    }
}

/// Moves `index` to the end of what is staged, written once.
pub(super) fn restage(written: &mut Vec<Staged>, index: ControlIndex) {
    written.retain(|earlier| !matches!(earlier, Staged::Control(staged) if *staged == index));
    written.push(Staged::Control(index));
}

fn refused(table: &ControlTable, index: ControlIndex, refusal: Refusal) -> String {
    let key = table.key(index).unwrap_or_default();
    match refusal {
        Refusal::Invalid => format!("Control '{key}' cannot hold that value"),
        _ => format!("Control '{key}' was not applied ({refusal:?})"),
    }
}
