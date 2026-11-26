//! A batch of control writes on an offline render, made whole or refused
//! with nothing written, as a daemon's `SetControls` is.

use super::RenderEngine;
use crate::invention::declared::WriteOptions;
use crate::invention::runtime::acks::source;
use crate::invention::runtime::GraphCommandError;
use crate::rpc::{
    AppliedControl, ControlRequestState, ControlWriteReason, ControlWriteRefusal, RpcError,
};
use crate::ControlWrite;

impl RenderEngine {
    /// Makes `writes` in order, or none of them, as a daemon's
    /// `SetControls` does (see
    /// [`RunningInvention::write_controls`](crate::RunningInvention::write_controls)):
    /// the batch is checked whole first, and the first write the rule
    /// refuses refuses it, with the daemon's code, reason and message. A
    /// number outside the range its control holds is written as the nearer
    /// limit, and its entry says it was clamped. Each entry has the
    /// daemon's fields: an offline render applies a write as it is made,
    /// so each is `applied`, reading `value` back, unless it failed as it
    /// was made (refused, with the reason) or lost to a higher-ranked
    /// write (`overridden`).
    ///
    /// A render writes now: a write with a `when` or a `ttl_ms` is refused.
    pub fn write_controls(&self, writes: &[ControlWrite]) -> Result<Vec<AppliedControl>, RpcError> {
        let timed = (writes.iter().enumerate())
            .find(|(_, write)| write.timing.when.is_some() || write.timing.ttl_ms.is_some());
        if let Some((index, write)) = timed {
            return Err(RpcError::invalid_control_write(ControlWriteRefusal {
                index,
                module_id: write.module_id.clone(),
                key: write.key.clone(),
                reason: ControlWriteReason::InvalidTiming,
                message: "an offline render writes now; `when` and `ttl_ms` do not apply".into(),
            }));
        }
        let snapshot = self.snapshot();
        let checked = snapshot.check_writes(writes)?;
        let mut controls = Vec::with_capacity(writes.len());
        for (write, checked) in writes.iter().zip(checked) {
            let options = WriteOptions {
                priority: write.priority.unwrap_or(0),
                ..WriteOptions::performed(source(write.source))
            };
            let (module_id, key) = (write.module_id.as_str(), write.key.as_str());
            let value = checked.value.clone();
            let made = snapshot.submit_control(module_id, key, value, write.intent, options);
            let entry = AppliedControl::read_back(module_id, key, None);
            controls.push(match made {
                Ok(ack) if ack.lost => AppliedControl {
                    state: Some(ControlRequestState::Overridden),
                    requested: Some(ack.value),
                    clamped: checked.clamped,
                    ..entry
                },
                Ok(ack) => AppliedControl {
                    value: snapshot.get_control(module_id, key).ok(),
                    state: Some(ControlRequestState::Applied),
                    requested: Some(ack.value),
                    clamped: checked.clamped,
                    ..entry
                },
                Err(error) => AppliedControl {
                    state: Some(ControlRequestState::Refused),
                    requested: Some(checked.value),
                    clamped: checked.clamped,
                    reason: Some(match error {
                        GraphCommandError::ControlError(message) => message,
                        other => other.to_string(),
                    }),
                    ..entry
                },
            });
        }
        Ok(controls)
    }
}
