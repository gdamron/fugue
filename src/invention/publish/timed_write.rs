//! Control writes timed on the wire: when one applies, resolved against
//! the graph playing as it is submitted (see `declared::DeclaredSurface`'s
//! live route).
//!
//! A timed write takes no part in a timed edit's keep-or-cancel (see
//! `Publisher::keep_authored`): whichever of the two applies later is what
//! plays, and an authored one is recorded as it applies, so the document
//! follows the same order. A commit's fence forgets one only for a module
//! it rebuilt or removed (see `authored::Fence`); one to a control a commit
//! writes is ordered against that write by recording both as they apply
//! (see `Committed::contested`).
//!
//! A write timed on a sample waits in the pending store, which applies
//! back-pressure when full: what follows waits until room frees. So the
//! front doors keep at most [`SAMPLE_TIMED_CAPACITY`] of them waiting, and
//! a phrase written far ahead never holds up every write behind it.

use super::publisher::PENDING_REQUEST_CAPACITY;
use super::timed_status::EditState;
use super::{PendingLog, Publisher};
use crate::control_request::{BeatSpec, BeatTime, Condition, Outcome, Predicate, RequestId, When};
use crate::invention::request_log::LoggedBeat;

/// The most control writes timed on a sample kept waiting at once: half the
/// pending store, the rest left for what applies within a block.
pub(crate) const SAMPLE_TIMED_CAPACITY: usize = PENDING_REQUEST_CAPACITY / 2;

/// When a timed control write applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WriteTime<'a> {
    /// On this sample of the transport.
    AtSample(u64),
    /// This many samples after the audio thread takes it.
    AfterSamples(u64),
    /// On beat `spec` of clock `clock`, or of the written module's clock.
    Beat {
        clock: Option<&'a str>,
        spec: BeatSpec,
    },
    /// `offset` samples after request `request` applies: a control write's
    /// or a timed edit's.
    After { request: RequestId, offset: u32 },
}

/// A timed write resolved: its time, and its beat for the request log.
pub(crate) struct Timed {
    pub(crate) when: When,
    pub(crate) beat: Option<LoggedBeat>,
}

/// Resolves `time` for a write to module `module_id`, under `publisher`
/// and the log, both locked as the write is submitted, every outcome
/// received. `conditions` says whether a condition can be submitted (room
/// among the watches, a value rather than a payload). One on a sample is
/// refused while [`SAMPLE_TIMED_CAPACITY`] such writes wait.
pub(crate) fn resolve(
    time: WriteTime<'_>,
    module_id: &str,
    (publisher, pending): (&mut Publisher, &mut PendingLog),
    conditions: Result<(), &str>,
) -> Result<Timed, String> {
    let timed = time_of(time, module_id, (publisher, pending), conditions)?;
    let on_sample = matches!(timed.when, When::AtSample(_) | When::AfterSamples(_));
    if on_sample && pending.on_sample() >= SAMPLE_TIMED_CAPACITY {
        return Err(format!(
            "{SAMPLE_TIMED_CAPACITY} writes already wait on a sample, as many as may at once; \
             write it once some have applied"
        ));
    }
    Ok(timed)
}

fn time_of(
    time: WriteTime<'_>,
    module_id: &str,
    (publisher, pending): (&mut Publisher, &mut PendingLog),
    conditions: Result<(), &str>,
) -> Result<Timed, String> {
    let at = |when| Timed { when, beat: None };
    match time {
        WriteTime::AtSample(sample) => Ok(at(When::AtSample(sample))),
        WriteTime::AfterSamples(samples) => Ok(at(When::AfterSamples(samples))),
        WriteTime::Beat { clock, spec } => {
            let index = (publisher.beat_clock(module_id, clock)).map_err(|e| e.to_string())?;
            let mirror = &publisher.mirror().modules;
            let id = mirror.get_index(index as usize).map(|(id, _)| id.as_str());
            let when = When::Beat(BeatTime {
                clock: index,
                spec,
                ramp: None,
            });
            let beat = id.map(|id| super::timed::logged_beat(id, spec));
            Ok(Timed { when, beat })
        }
        WriteTime::After { request, offset } => {
            let after = |sample: u64| at(When::AtSample(sample.saturating_add(offset.into())));
            let waits = |conditions: Result<(), &str>| {
                conditions.map_err(str::to_string)?;
                let applied = Condition::all(&[Predicate::Applied(request)]).expect("one part");
                Ok(at(When::Condition(applied.delayed(offset))))
            };
            let never = || format!("request {} never applied, so nothing follows it", request.0);
            if let Some(outcome) = pending.outcome_of(request) {
                return match outcome {
                    Outcome::Applied { at } | Outcome::AppliedLate { at, .. } => Ok(after(at)),
                    _ => Err(never()),
                };
            }
            if pending.waiting(request) {
                return waits(conditions);
            }
            match publisher.ledger().status(request) {
                Some(status) => match (status.state, status.sample) {
                    (EditState::Queued, _) => waits(conditions),
                    (EditState::Landed | EditState::Adopted, Some(sample)) => Ok(after(sample)),
                    _ => Err(never()),
                },
                None => Err(format!("no request {} is known to follow", request.0)),
            }
        }
    }
}
