//! Control writes from a front door that tells its caller what became of
//! each: applied, still pending, or lost and why.

use super::control_timing::timing_time;
use super::{GraphCommandError, RunningInvention};
use crate::control_request::{Outcome, Refusal, Source};
use crate::invention::declared::WriteOptions;
use crate::invention::orchestration::ControlAck;
use crate::invention::publish::Progress;
use crate::invention::write_rule::CheckedWrite;
use crate::rpc::{AppliedControl, ControlRequestState, RequestSource, RpcError};
use crate::ControlWrite;

/// How many device blocks a reply waits for its writes to apply: one
/// queued as a block starts applies by the next, so this covers a busy
/// control thread too.
#[cfg(not(target_arch = "wasm32"))]
const ACK_BLOCKS: f64 = 4.0;

/// Bounds on the wait, in milliseconds, whatever the device's block.
#[cfg(not(target_arch = "wasm32"))]
const ACK_WAIT_MS: (f64, f64) = (20.0, 200.0);

/// What [`RunningInvention::write_controls`] reports.
#[derive(Debug)]
pub struct ControlAcks {
    /// Each write, in order, and what became of it: every write of a batch
    /// the rule accepts (see `write_rule`), or none.
    pub controls: Vec<AppliedControl>,
    /// Why the batch was refused, with nothing written: the first write
    /// the rule refuses ([`crate::rpc::RpcErrorCode::InvalidControlWrite`]).
    pub error: Option<RpcError>,
}

impl RunningInvention {
    /// Makes `writes` in order, each from its source at its priority, and
    /// reports what became of each; or makes none of them, when the rule
    /// refuses one (see `write_rule`): the batch is checked whole, timed
    /// writes and untimed alike, before any is submitted. A number outside
    /// the range its control holds is written as the nearer limit, and its
    /// entry says it was clamped. A write the rule accepts can still fail
    /// as it is made, on what only making it shows (a sample that does not
    /// load, an audio thread with no room): its entry is refused, with the
    /// reason, and the rest are made all the same.
    ///
    /// A reply waits a few device blocks for its writes to apply, never
    /// more than 200 ms, and not at all with no device playing (a headless
    /// or stalled stream): one not applied by then is reported pending, and
    /// announced as a `ControlChanged` event naming its request if it
    /// applies later.
    pub fn write_controls(&self, writes: &[ControlWrite]) -> ControlAcks {
        let snapshot = self.snapshot();
        let checked = match snapshot.check_writes(writes) {
            Ok(checked) => checked,
            Err(error) => {
                let controls = Vec::new();
                return ControlAcks {
                    controls,
                    error: Some(error),
                };
            }
        };
        let mut made = Vec::with_capacity(writes.len());
        // Their outcomes are held apart from the shared ring, which a
        // large batch or other writers could wrap, until the reply is built.
        self.live.hold_room(writes.len());
        for (write, checked) in writes.iter().zip(checked) {
            let submitted = timing_time(&write.timing).and_then(|time| {
                // Held for the reply, timed or not: a reply waits only for
                // one due now (see below), but reads what became of each.
                let options = WriteOptions {
                    priority: write.priority.unwrap_or(0),
                    ttl: self.ttl_samples(write.timing.ttl_ms),
                    time,
                    ..WriteOptions::performed(source(write.source)).awaiting()
                };
                let (module_id, key) = (&write.module_id, &write.key);
                let value = checked.value.clone();
                snapshot.submit_control(module_id, key, value, write.intent, options)
            });
            made.push((write, checked, submitted));
        }
        let due_now = (made.iter())
            .filter(|(write, ..)| write.timing.when.is_none())
            .filter_map(|(_, _, submitted)| submitted.as_ref().ok());
        self.await_acks(due_now);
        snapshot.announce_applied();
        let held: Vec<_> = (made.iter())
            .filter_map(|(_, _, submitted)| submitted.as_ref().ok()?.request)
            .collect();
        let controls = (made.into_iter())
            .map(|(write, checked, submitted)| match submitted {
                Ok(ack) => self.acknowledged(write, ack, checked.clamped),
                Err(error) => failed(write, checked, error),
            })
            .collect();
        self.live.release(&held);
        ControlAcks {
            controls,
            error: None,
        }
    }

    /// Announces each front door's write that applied since the last call
    /// (see [`crate::RuntimeSnapshot::announce_applied`]). A host calls it
    /// now and then, so a write that applied after its reply is announced.
    pub fn announce_applied(&self) {
        self.snapshot().announce_applied();
    }

    /// Waits, as long as [`Self::ack_wait`] allows, until no write in
    /// `acks` is still waiting.
    fn await_acks<'a>(&self, acks: impl Iterator<Item = &'a ControlAck>) {
        let acks: Vec<_> = acks.collect();
        let waiting = || (acks.iter()).any(|ack| self.progress(ack) == Progress::Waiting);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let deadline = std::time::Instant::now() + self.ack_wait();
            while waiting() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = waiting;
    }

    /// How long a reply waits for its writes: [`ACK_BLOCKS`] of the
    /// device's, bounded, or none when no device is playing: none is
    /// attached, or none has asked for audio within the longest wait.
    #[cfg(not(target_arch = "wasm32"))]
    fn ack_wait(&self) -> std::time::Duration {
        let Some(diagnostics) = self.backend.diagnostics() else {
            return std::time::Duration::ZERO;
        };
        let diagnostics = diagnostics.snapshot();
        let (least, most) = ACK_WAIT_MS;
        // Stalled, a stream would take nothing in the longest wait.
        let playing = (diagnostics.last_callback_age_ms).is_some_and(|age| age < most);
        if !playing {
            return std::time::Duration::ZERO;
        }
        let wait = ACK_BLOCKS * diagnostics.buffer_period_ms;
        std::time::Duration::from_secs_f64(wait.clamp(least, most) / 1000.0)
    }

    /// How far a write taken as `ack` got.
    pub(super) fn progress(&self, ack: &ControlAck) -> Progress {
        match (ack.request, ack.published) {
            (Some(id), _) => self.live.progress(id),
            _ if ack.lost => Progress::Settled(Outcome::Overridden),
            (None, Some(generation)) if !self.live.installed(generation) => Progress::Waiting,
            (None, _) => Progress::Applied,
        }
    }

    /// `write`, taken as `ack`, as its reply reports it.
    fn acknowledged(&self, write: &ControlWrite, ack: ControlAck, clamped: bool) -> AppliedControl {
        let (state, reason) = state_of(self.progress(&ack));
        let landing = write.timing.when.is_some().then(|| self.landing_of(&ack));
        let value = (state == ControlRequestState::Applied)
            .then(|| self.get_control(&write.module_id, &write.key).ok())
            .flatten();
        AppliedControl {
            module_id: write.module_id.clone(),
            key: write.key.clone(),
            value,
            request: ack.request.map(|id| id.0.to_string()),
            state: Some(state),
            requested: Some(ack.value),
            clamped,
            reason,
            landing,
        }
    }
}

/// `write`, which the rule took as `checked` but which failed as it was
/// made, as its reply reports it.
fn failed(write: &ControlWrite, checked: CheckedWrite, error: GraphCommandError) -> AppliedControl {
    let reason = match error {
        GraphCommandError::ControlError(message) => message,
        other => other.to_string(),
    };
    AppliedControl {
        state: Some(ControlRequestState::Refused),
        requested: Some(checked.value),
        clamped: checked.clamped,
        reason: Some(reason),
        ..AppliedControl::read_back(&write.module_id, &write.key, None)
    }
}

/// What a request that got as far as `progress` reports, and why it was
/// refused, if it was.
pub(crate) fn state_of(progress: Progress) -> (ControlRequestState, Option<String>) {
    match progress {
        Progress::Waiting => (ControlRequestState::Pending, None),
        Progress::Applied
        | Progress::Settled(Outcome::Applied { .. } | Outcome::AppliedLate { .. }) => {
            (ControlRequestState::Applied, None)
        }
        Progress::Settled(Outcome::Superseded) => (ControlRequestState::Superseded, None),
        Progress::Settled(Outcome::Overridden) => (ControlRequestState::Overridden, None),
        Progress::Settled(Outcome::Refused(Refusal::Expired)) => {
            let reason = "its ttl ran out before it applied".to_string();
            (ControlRequestState::Expired, Some(reason))
        }
        Progress::Settled(Outcome::Refused(refusal)) => {
            (ControlRequestState::Refused, Some(refused(refusal)))
        }
    }
}

/// The tier a wire source submits at.
pub(crate) fn source(source: Option<RequestSource>) -> Source {
    match source.unwrap_or_default() {
        RequestSource::User => Source::User,
        RequestSource::Agent => Source::Agent,
    }
}

/// Why the audio thread refused a control write, for its reply.
fn refused(refusal: Refusal) -> String {
    match refusal {
        Refusal::TargetGone => "its module was removed or rebuilt before it applied".into(),
        Refusal::PendingFull => "the audio thread had no room for it".into(),
        Refusal::Unsupported => "its module does not take that write for this control".into(),
        Refusal::Invalid => "its value is not one the control holds".into(),
        Refusal::Stopped => "the invention it was queued on stopped, or its audio failed".into(),
        other => format!("refused by the audio thread ({other:?})"),
    }
}

#[cfg(test)]
#[test]
fn each_outcome_reports_its_state() {
    use ControlRequestState::*;
    let settled = |outcome| state_of(Progress::Settled(outcome));
    assert_eq!(state_of(Progress::Waiting), (Pending, None));
    assert_eq!(state_of(Progress::Applied), (Applied, None));
    assert_eq!(settled(Outcome::Applied { at: 0 }), (Applied, None));
    let late = Outcome::AppliedLate { at: 64, due: 0 };
    assert_eq!(settled(late), (Applied, None));
    assert_eq!(settled(Outcome::Superseded), (Superseded, None));
    assert_eq!(settled(Outcome::Overridden), (Overridden, None));
    let (state, reason) = settled(Outcome::Refused(Refusal::TargetGone));
    assert_eq!(state, Refused);
    assert!(reason.unwrap().contains("removed or rebuilt"));
    let (state, reason) = settled(Outcome::Refused(Refusal::Expired));
    assert_eq!(state, Expired);
    assert!(reason.unwrap().contains("ttl"));
}
