//! Timed control writes from a front door, in the wire's terms (see
//! [`crate::rpc::ControlTiming`]): when one applies, where its reply and
//! the pending listing say it lands, and what its settled event reports.

use super::acks::state_of;
use super::timed::{edit_time, parse_id};
use super::{GraphCommandError, RunningInvention};
use crate::control_request::{Outcome, When};
use crate::invention::orchestration::ControlAck;
use crate::invention::publish::{Announcement, EditTime, Progress, WriteTime};
use crate::invention::request_log::LoggedBeat;
use crate::rpc::{ControlRequestSettled, ControlTiming, ControlWhen, EditWhen, RequestLanding};

/// The time `timing` names, if any. A ttl bounds only a write with a
/// `when`: one without is refused with nothing sent, as every front door
/// refuses it.
pub(crate) fn timing_time(
    timing: &ControlTiming,
) -> Result<Option<WriteTime<'_>>, GraphCommandError> {
    match (&timing.when, timing.ttl_ms) {
        (None, Some(_)) => Err(GraphCommandError::ControlError(
            "`ttl_ms` applies only with a `when`; nothing was sent".into(),
        )),
        (when, _) => when.as_ref().map(write_time).transpose(),
    }
}

/// The time `when` names. A beat is checked as a timed edit's is (see
/// [`edit_time`]), counting on the clock it names, otherwise on the written
/// module's.
fn write_time(when: &ControlWhen) -> Result<WriteTime<'_>, GraphCommandError> {
    let refuse = |message: String| GraphCommandError::ControlError(message);
    let (clock, beat) = match when {
        ControlWhen::AtSample { sample } => return Ok(WriteTime::AtSample(*sample)),
        ControlWhen::AfterSamples { samples } => return Ok(WriteTime::AfterSamples(*samples)),
        ControlWhen::AfterRequest {
            request,
            offset_samples,
        } => {
            let id = parse_id(request);
            let id =
                id.ok_or_else(|| refuse(format!("no request {request} is known to follow")))?;
            let offset = u32::try_from(*offset_samples)
                .map_err(|_| refuse("`offset_samples` is too large".into()))?;
            return Ok(WriteTime::After {
                request: id,
                offset,
            });
        }
        ControlWhen::AfterBeats { beats, clock } => (
            clock,
            EditWhen::AfterBeats {
                beats: *beats,
                clock: None,
            },
        ),
        ControlWhen::NextBeat {
            every,
            offset,
            clock,
        } => (
            clock,
            EditWhen::NextBeat {
                every: *every,
                offset: *offset,
                clock: None,
            },
        ),
        ControlWhen::NextBar {
            beats_per_bar,
            clock,
        } => (
            clock,
            EditWhen::NextBar {
                beats_per_bar: *beats_per_bar,
                clock: None,
            },
        ),
    };
    match edit_time(&beat, None)? {
        EditTime::FirstClock(spec) => Ok(WriteTime::Beat {
            clock: clock.as_deref(),
            spec,
        }),
        _ => Err(refuse("a beat time counts on a clock".into())),
    }
}

/// Where a write timed `when` (with `beat`, timed in beats) lands, as far
/// as `outcome` says: the sample it applied on, or is timed to, and the
/// one it was due on when it applied late.
pub(super) fn landing(
    when: Option<When>,
    beat: Option<LoggedBeat>,
    outcome: Option<Outcome>,
) -> RequestLanding {
    let (sample, due) = match (outcome, when) {
        (Some(Outcome::Applied { at }), _) => (Some(at), None),
        (Some(Outcome::AppliedLate { at, due }), _) => (Some(at), Some(due)),
        (None, Some(When::AtSample(sample))) => (Some(sample), None),
        _ => (None, None),
    };
    RequestLanding { sample, due, beat }
}

/// A timed write's settled event, for one that `applied` announces; `None`
/// for an untimed write.
pub(crate) fn settled(applied: &Announcement) -> Option<ControlRequestSettled> {
    let beat = applied.timed.clone()?;
    let progress = applied.outcome.map_or(Progress::Applied, Progress::Settled);
    let (state, reason) = state_of(progress);
    Some(ControlRequestSettled {
        request: applied.request.0.to_string(),
        module_id: applied.module_id.clone(),
        key: applied.key.clone(),
        state,
        landing: landing(None, beat, applied.outcome),
        reason,
    })
}

impl RunningInvention {
    /// Where a timed write taken as `ack` lands, as its reply says.
    pub(super) fn landing_of(&self, ack: &ControlAck) -> RequestLanding {
        match self.progress(ack) {
            Progress::Waiting => {
                let id = ack.request;
                let waiting = (self.live.pending_requests().into_iter())
                    .find(|(pending, _)| Some(*pending) == id);
                waiting.map_or_else(RequestLanding::default, |(_, write)| {
                    landing(Some(write.when), write.beat, None)
                })
            }
            Progress::Settled(outcome) => landing(None, None, Some(outcome)),
            Progress::Applied => RequestLanding::default(),
        }
    }
}
