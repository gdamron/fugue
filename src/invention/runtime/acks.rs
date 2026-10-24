//! Control writes from a front door that tells its caller what became of
//! each: applied, still pending, or lost and why.

use super::{GraphCommandError, RunningInvention};
use crate::control_request::{Outcome, Refusal, Source};
use crate::invention::declared::WriteOptions;
use crate::invention::orchestration::ControlAck;
use crate::invention::publish::Progress;
use crate::rpc::{AppliedControl, ControlRequestState, RequestSource};
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
    /// Each write submitted, in order, and what became of it.
    pub controls: Vec<AppliedControl>,
    /// The write that could not be submitted, which stopped the batch:
    /// those before it stand, as control writes have no rollback.
    pub error: Option<GraphCommandError>,
}

impl RunningInvention {
    /// Makes `writes` in order, as [`crate::OrchestrationRuntime::set_controls`]
    /// does, each from its source at its priority, and reports what became
    /// of each. A reply waits a few device blocks for its writes to apply,
    /// never more than 200 ms, and not at all with no device playing (a
    /// headless or stalled stream): one not applied by then is reported
    /// pending, and announced as a `ControlChanged` event naming its
    /// request if it applies later.
    pub fn write_controls(&self, writes: &[ControlWrite]) -> ControlAcks {
        let snapshot = self.snapshot();
        let mut taken = Vec::with_capacity(writes.len());
        let mut error = None;
        for write in writes {
            let options = WriteOptions {
                priority: write.priority.unwrap_or(0),
                ..WriteOptions::performed(source(write.source))
            };
            let (module_id, key) = (&write.module_id, &write.key);
            let value = write.value.clone();
            match snapshot.submit_control(module_id, key, value, write.intent, options) {
                Ok(ack) => taken.push((write, ack)),
                Err(refusal) => {
                    error = Some(refusal);
                    break;
                }
            }
        }
        self.await_acks(taken.iter().map(|(_, ack)| ack));
        snapshot.announce_applied();
        let controls = (taken.into_iter())
            .map(|(write, ack)| self.acknowledged(write, ack))
            .collect();
        ControlAcks { controls, error }
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
    fn progress(&self, ack: &ControlAck) -> Progress {
        match (ack.request, ack.published) {
            (Some(id), _) => self.live.progress(id),
            _ if ack.lost => Progress::Settled(Outcome::Overridden),
            (None, Some(generation)) if !self.live.installed(generation) => Progress::Waiting,
            (None, _) => Progress::Applied,
        }
    }

    /// `write`, taken as `ack`, as its reply reports it.
    fn acknowledged(&self, write: &ControlWrite, ack: ControlAck) -> AppliedControl {
        let (state, reason) = state_of(self.progress(&ack));
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
            reason,
        }
    }
}

/// What a request that got as far as `progress` reports, and why it was
/// refused, if it was.
fn state_of(progress: Progress) -> (ControlRequestState, Option<String>) {
    match progress {
        Progress::Waiting => (ControlRequestState::Pending, None),
        Progress::Applied
        | Progress::Settled(Outcome::Applied { .. } | Outcome::AppliedLate { .. }) => {
            (ControlRequestState::Applied, None)
        }
        Progress::Settled(Outcome::Superseded) => (ControlRequestState::Superseded, None),
        Progress::Settled(Outcome::Overridden) => (ControlRequestState::Overridden, None),
        Progress::Settled(Outcome::Refused(refusal)) => {
            (ControlRequestState::Refused, Some(refused(refusal)))
        }
    }
}

/// The tier a wire source submits at.
fn source(source: Option<RequestSource>) -> Source {
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
}
