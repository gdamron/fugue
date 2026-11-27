//! A batch of control writes on an offline render, made whole or refused
//! with nothing written, as a daemon's `SetControls` is.

use super::RenderEngine;
use crate::invention::declared::{WriteOptions, AFTER_UNTIMED, AUTHORED_UNTIMED};
use crate::invention::orchestration::{ControlAck, RuntimeSnapshot};
use crate::invention::publish::timed_write::{self, BatchTime};
use crate::invention::publish::TopologyMirror;
use crate::invention::runtime::acks::source;
use crate::invention::runtime::control_timing::{landing, timing_time, ttl_samples};
use crate::invention::runtime::GraphCommandError;
use crate::invention::write_rule::CheckedWrite;
use crate::rpc::{
    AppliedControl, ControlRequestState, ControlWhen, ControlWriteIntent, ControlWriteReason,
    ControlWriteRefusal, RequestLanding, RpcError,
};
use crate::ControlWrite;

impl RenderEngine {
    /// Makes `writes` in order, or none of them, as a daemon's
    /// `SetControls` does (see
    /// [`RunningInvention::write_controls`](crate::RunningInvention::write_controls)):
    /// the batch is checked whole first, timed writes and untimed alike,
    /// and the first write the rule refuses refuses it, with the daemon's
    /// code, reason and message. A number outside the range its control
    /// holds is written as the nearer limit, and its entry says it was
    /// clamped. Each entry has the daemon's fields: an offline render
    /// applies an untimed write as it is made, so each is `applied`,
    /// reading `value` back, unless it failed as it was made (refused,
    /// with the reason) or lost to a higher-ranked write (`overridden`).
    ///
    /// A write with a `when` waits, `pending`, with its request id and
    /// where it lands, and applies where the daemon's would: on its
    /// sample, or on its beat as the render's clock plays it, as later
    /// renders reach it (see [`Self::take_settled_writes`]). Its `ttl_ms`
    /// bounds how late it may apply, as the daemon's does. One timed after
    /// another request (`after_request`), or an authored one timed (only a
    /// `perform` write can be timed here yet), refuses the batch, nothing
    /// written.
    pub fn write_controls(&self, writes: &[ControlWrite]) -> Result<Vec<AppliedControl>, RpcError> {
        // The daemon's checks first, so a batch the daemon would refuse is
        // refused at the same write, with the same message, up to one timed
        // after another request: this render can follow none, whatever
        // request it names, so it is refused as such.
        let snapshot = self.snapshot();
        let after = (writes.iter())
            .position(|write| matches!(write.timing.when, Some(ControlWhen::AfterRequest { .. })));
        let prefix = &writes[..after.unwrap_or(writes.len())];
        let checked = snapshot.check_writes(prefix)?;
        self.check_write_times(&snapshot, prefix)?;
        if let Some(index) = after {
            return Err(timing_refused(writes, index, AFTER_UNTIMED.into()));
        }
        self.settle_timed_writes();
        let mut controls = Vec::with_capacity(writes.len());
        for (write, checked) in writes.iter().zip(checked) {
            let submitted = timing_time(&write.timing).and_then(|time| {
                let options = WriteOptions {
                    priority: write.priority.unwrap_or(0),
                    ttl: ttl_samples(self.sample_rate, write.timing.ttl_ms),
                    time,
                    ..WriteOptions::performed(source(write.source))
                };
                let (module_id, key) = (write.module_id.as_str(), write.key.as_str());
                let value = checked.value.clone();
                snapshot.submit_control(module_id, key, value, write.intent, options)
            });
            controls.push(match submitted {
                Ok(ack) => self.acknowledged(write, ack, &checked),
                Err(error) => refused(write, checked, error),
            });
        }
        Ok(controls)
    }

    /// Whether each timed write of `writes` would be placed now, as the
    /// daemon checks a batch's before making any: the first that would
    /// not refuses the batch.
    fn check_write_times(
        &self,
        snapshot: &RuntimeSnapshot,
        writes: &[ControlWrite],
    ) -> Result<(), RpcError> {
        // One to an event control is performed whatever its intent, as
        // `RuntimeSnapshot::submit_control` makes it.
        let authored = writes.iter().position(|write| {
            write.intent == ControlWriteIntent::Author
                && write.timing.when.is_some()
                && !snapshot.is_event(&write.module_id, &write.key)
        });
        if let Some(index) = authored {
            return Err(timing_refused(writes, index, AUTHORED_UNTIMED.into()));
        }
        let times: Vec<BatchTime<'_>> = (writes.iter().enumerate())
            .filter_map(|(index, write)| {
                let time = timing_time(&write.timing).ok()??;
                Some((index, write.module_id.as_str(), time, false))
            })
            .collect();
        let Some(graph) = self.graph.as_ref().filter(|_| !times.is_empty()) else {
            return Ok(());
        };
        let mirror = {
            let graph = graph.lock().unwrap();
            TopologyMirror::of(&graph.modules, &graph.edges)
        };
        let Err((index, message)) = timed_write::check_placed(&times, &mirror, AFTER_UNTIMED)
        else {
            return Ok(());
        };
        Err(timing_refused(writes, index, message))
    }

    /// `write`, taken as `ack`, as its reply reports it.
    fn acknowledged(
        &self,
        write: &ControlWrite,
        ack: ControlAck,
        checked: &CheckedWrite,
    ) -> AppliedControl {
        let (module_id, key) = (write.module_id.as_str(), write.key.as_str());
        let entry = AppliedControl {
            requested: Some(ack.value),
            clamped: checked.clamped,
            ..AppliedControl::read_back(module_id, key, None)
        };
        match ack.request {
            Some(id) => AppliedControl {
                request: Some(id.0.to_string()),
                state: Some(ControlRequestState::Pending),
                landing: Some(self.waiting_landing(id)),
                ..entry
            },
            None if ack.lost => AppliedControl {
                state: Some(ControlRequestState::Overridden),
                ..entry
            },
            None => AppliedControl {
                value: self.snapshot().get_control(module_id, key).ok(),
                state: Some(ControlRequestState::Applied),
                ..entry
            },
        }
    }

    /// Where timed write `id`, just queued, lands, as the daemon's reply
    /// says: its sample, or its beat.
    fn waiting_landing(&self, id: crate::control_request::RequestId) -> RequestLanding {
        let Some(graph) = &self.graph else {
            return RequestLanding::default();
        };
        let graph = graph.lock().unwrap();
        let waiting = graph.offline.as_ref().and_then(|o| o.waiting_write(id));
        waiting.map_or_else(RequestLanding::default, |write| {
            landing(Some(write.when), write.beat.clone(), None)
        })
    }
}

/// The batch `writes`, refused for the timing of write `index`.
fn timing_refused(writes: &[ControlWrite], index: usize, message: String) -> RpcError {
    let write = &writes[index];
    RpcError::invalid_control_write(ControlWriteRefusal {
        index,
        module_id: write.module_id.clone(),
        key: write.key.clone(),
        reason: ControlWriteReason::InvalidTiming,
        message,
    })
}

/// `write`, which the rule took as `checked` but which failed as it was
/// made, as its reply reports it.
fn refused(
    write: &ControlWrite,
    checked: CheckedWrite,
    error: GraphCommandError,
) -> AppliedControl {
    AppliedControl {
        state: Some(ControlRequestState::Refused),
        requested: Some(checked.value),
        clamped: checked.clamped,
        reason: Some(match error {
            GraphCommandError::ControlError(message) => message,
            other => other.to_string(),
        }),
        ..AppliedControl::read_back(&write.module_id, &write.key, None)
    }
}
