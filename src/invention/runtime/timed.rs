//! Timed structural edits on a running invention: a swap, an edit batch or
//! a reload queued to land on a sample or a beat, and what became of it,
//! in the wire's terms (see [`crate::rpc::timed`]).

use super::{GraphCommandError, RunningInvention};
use crate::control_request::Refusal;
use crate::control_request::{BeatSpec, RequestId};
use crate::invention::handles::InventionHandles;
use crate::invention::publish::{CancelCause, EditState, EditStatus, EditTime, Touched};
use crate::rpc::{EditWhen, RequestLanding, RequestState, RequestStatus};

impl RunningInvention {
    /// [`Self::swap_module`], landing `when` rather than at once, unless
    /// `ttl_ms` milliseconds pass first. Returns the queued request's
    /// status.
    ///
    /// A beat time counts on the clock it names, otherwise on the one the
    /// swapped module counts on. The replacement is built now, so an
    /// unknown type or a bad config is reported at once, as is a time that
    /// can never come (a clock that is not one, a negative beat).
    ///
    /// While it waits, any other structural edit, reload or edit batch
    /// cancels it, as does an authored write to the module it swaps; an
    /// authored write to another module is kept, and performed writes never
    /// touch it. Follow it with [`Self::request_status`] or
    /// [`Self::take_request_changes`], and take the built module's handles
    /// with [`Self::request_handles`] once it is adopted.
    pub fn swap_module_at(
        &self,
        module_id: &str,
        module_type: &str,
        config: &serde_json::Value,
        preserve_connections: bool,
        when: &EditWhen,
        ttl_ms: Option<u64>,
    ) -> Result<RequestStatus, GraphCommandError> {
        let time = edit_time(when, Some(module_id))?;
        let ttl = self.ttl_samples(ttl_ms);
        let id = self.live.swap_module_at(
            self.sample_rate,
            module_id,
            module_type,
            config,
            preserve_connections,
            time,
            ttl,
        )?;
        Ok(self.status_of(id))
    }

    /// Timed request `id`'s status, just queued.
    pub(crate) fn status_of(&self, id: RequestId) -> RequestStatus {
        let status = self.live.timed_status(id);
        // Forgotten only once dozens more were queued since.
        status.map_or_else(|| forgotten(id), wire_status)
    }

    /// `ttl_ms` in samples.
    pub(crate) fn ttl_samples(&self, ttl_ms: Option<u64>) -> Option<u64> {
        ttl_ms.map(|ms| ms.saturating_mul(u64::from(self.sample_rate)) / 1000)
    }

    /// Where timed request `id` stands, while the runtime remembers it (its
    /// latest few dozen).
    pub fn request_status(&self, id: &str) -> Option<RequestStatus> {
        self.live.timed_status(parse_id(id)?).map(wire_status)
    }

    /// Every timed request whose status changed since the last call, at its
    /// latest, in the order they last changed.
    pub fn take_request_changes(&self) -> Vec<RequestStatus> {
        let changes = self.live.take_timed_changes();
        changes.into_iter().map(wire_status).collect()
    }

    /// Settles the timed edit waiting, if any, before the invention stops
    /// or its state is saved for the last time: cancels it, or adopts it
    /// when it has already landed, so that the document read next holds
    /// everything that played and nothing lands after.
    pub fn settle_requests(&self) {
        self.live.settle_timed();
    }

    /// The handles of the modules timed request `id` built, once it is
    /// adopted; taken once.
    pub fn request_handles(&self, id: &str) -> Option<InventionHandles> {
        let committed = self.live.take_timed_committed(parse_id(id)?)?;
        Some(self.follow_up(committed))
    }
}

fn parse_id(id: &str) -> Option<RequestId> {
    id.parse().ok().map(RequestId)
}

/// The time `when` names, a beat counting on its clock or else on the clock
/// `module` counts on, or the invention's first clock without one. Refuses
/// beats that are not finite and not negative, and grids whose step is not
/// positive.
pub(crate) fn edit_time<'a>(
    when: &'a EditWhen,
    module: Option<&'a str>,
) -> Result<EditTime<'a>, GraphCommandError> {
    let refuse = |message: &str| Err(GraphCommandError::ControlError(message.to_string()));
    let (clock, spec) = match when {
        EditWhen::AtSample { sample } => return Ok(EditTime::AtSample(*sample)),
        EditWhen::AfterSamples { samples } => return Ok(EditTime::AfterSamples(*samples)),
        EditWhen::AfterBeats { beats, clock } => {
            if !(beats.is_finite() && *beats >= 0.0) {
                return refuse("`after_beats` needs a finite number of beats, not negative");
            }
            (clock, BeatSpec::After(*beats))
        }
        EditWhen::NextBeat {
            every,
            offset,
            clock,
        } => {
            if !(every.is_finite() && *every > 0.0 && offset.is_finite() && *offset >= 0.0) {
                return refuse("`next_beat` needs a positive `every` and an `offset` not negative");
            }
            (clock, grid(*every, *offset))
        }
        EditWhen::NextBar {
            beats_per_bar,
            clock,
        } => {
            if !(beats_per_bar.is_finite() && *beats_per_bar > 0.0) {
                return refuse("`next_bar` needs a positive `beats_per_bar`");
            }
            (clock, grid(*beats_per_bar, 0.0))
        }
    };
    Ok(match (clock, module) {
        (Some(clock), _) => EditTime::Beat { clock, spec },
        (None, Some(module)) => EditTime::BeatOf { module, spec },
        (None, None) => EditTime::FirstClock(spec),
    })
}

/// The next beat `offset` past a multiple of `every`, once.
fn grid(every: f32, offset: f32) -> BeatSpec {
    BeatSpec::Grid {
        every,
        offset,
        repeat: false,
    }
}

/// A request forgotten, as far as anyone can tell, still waiting.
fn forgotten(id: RequestId) -> RequestStatus {
    RequestStatus {
        id: id.0.to_string(),
        state: RequestState::Queued,
        landing: RequestLanding::default(),
        reason: None,
        built: Vec::new(),
        removed: Vec::new(),
    }
}

/// `status` in the wire's terms.
pub(crate) fn wire_status(status: EditStatus) -> RequestStatus {
    let (state, reason) = match status.state {
        EditState::Queued => (RequestState::Queued, None),
        EditState::Landed => (RequestState::Landed, None),
        EditState::Adopted => (RequestState::Adopted, None),
        EditState::Expired => {
            let reason = "its ttl passed before its time came".to_string();
            (RequestState::Expired, Some(reason))
        }
        EditState::Refused(refusal) => (RequestState::Refused, Some(refused(refusal))),
        EditState::Cancelled(cause) => (RequestState::Cancelled, Some(cancelled(&cause))),
        EditState::Lost => {
            let reason = "its outcome was lost (too many outcomes at once); it did not land";
            (RequestState::Refused, Some(reason.to_string()))
        }
    };
    RequestStatus {
        id: status.id.0.to_string(),
        state,
        landing: RequestLanding {
            sample: status.sample,
            due: status.due,
            beat: status.beat,
        },
        reason,
        built: status.built,
        removed: status.removed,
    }
}

fn refused(refusal: Refusal) -> String {
    match refusal {
        Refusal::TargetGone => "the graph changed before it landed".to_string(),
        Refusal::TimelineGone => "its clock was removed or rebuilt before it landed".to_string(),
        Refusal::NoTimeline => "it was timed in beats on a module that is not a clock".to_string(),
        Refusal::NoClock => "it was timed by wall clock, and there is none".to_string(),
        Refusal::SourceGone => "a module its condition reads was removed or rebuilt".to_string(),
        Refusal::ChainBroken => "the request it waited on did not apply".to_string(),
        Refusal::PendingFull => "the audio thread had no room for it".to_string(),
        other => format!("refused by the audio thread ({other:?})"),
    }
}

fn cancelled(cause: &CancelCause) -> String {
    match cause {
        CancelCause::Commit => {
            "cancelled: another structural edit, reload or edit batch committed first".to_string()
        }
        CancelCause::AuthoredWrite {
            module_id,
            key,
            touches,
        } => {
            let does = match touches {
                Touched::Removed => "whose module this edit removes",
                Touched::Rebuilt => "whose module this edit rebuilds",
                Touched::Written => "which this edit also writes as it lands",
            };
            format!("cancelled: authored write to {module_id}.{key}, {does}")
        }
        CancelCause::ScheduleWrite { module_id } => {
            format!("cancelled: a schedule written to {module_id}, which this edit re-resolves")
        }
        CancelCause::Unchained => {
            "cancelled: the request queue had no room for the control writes it makes".to_string()
        }
        CancelCause::Stopping => {
            "cancelled: the invention it was queued on is stopping".to_string()
        }
    }
}
