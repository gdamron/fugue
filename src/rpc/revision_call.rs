//! The daemon command a browser host's call stands for under the revision
//! rule: built from the call's parts (its tool, its control writes, an edit
//! batch's timing), so a host that keeps its own revision asks the same
//! [`RpcCommand::advances_revision_after`] without the engine parsing whole
//! commands it never runs.

use super::{ControlWrite, EditTiming, EditWhen, RpcCommand, RpcError, RpcErrorCode};

impl RpcCommand {
    /// The command a host's `command` call stands for, for
    /// [`Self::advances_revision_after`]: `set_control` with its one write,
    /// `set_controls` with its batch, or `apply_edits`, `timed` when it has
    /// a `when` (a timed batch advances as it is adopted, not as it is
    /// queued). The rule reads neither an edit batch's edits nor when it
    /// lands, only whether it is timed, so the batch carries no edits and a
    /// stand-in time.
    ///
    /// Refused as `invalid_request`: a `set_control` without exactly one
    /// write, writes given to an edit batch, `timed` given to a control
    /// write (it carries its own timing), and any other command.
    pub fn revision_call(
        command: &str,
        writes: Vec<ControlWrite>,
        timed: bool,
    ) -> Result<Self, RpcError> {
        let refuse = |message: String| Err(RpcError::new(RpcErrorCode::InvalidRequest, message));
        match command {
            "set_control" | "set_controls" if timed => {
                refuse(format!("a {command} carries its timing on its writes"))
            }
            "set_control" => match <[ControlWrite; 1]>::try_from(writes) {
                Ok([write]) => Ok(Self::SetControl {
                    module_id: write.module_id,
                    key: write.key,
                    value: write.value,
                    intent: write.intent,
                    source: write.source,
                    priority: write.priority,
                    timing: write.timing,
                }),
                Err(writes) => refuse(format!(
                    "a set_control makes one write, not {}",
                    writes.len()
                )),
            },
            "set_controls" => Ok(Self::SetControls { writes }),
            "apply_edits" if !writes.is_empty() => {
                refuse("an apply_edits carries no control writes".to_string())
            }
            "apply_edits" => Ok(Self::ApplyEdits {
                edits: Vec::new(),
                timing: EditTiming {
                    when: timed.then_some(EditWhen::AfterSamples { samples: 0 }),
                    ttl_ms: None,
                },
            }),
            other => refuse(format!("no revision rule for `{other}` here")),
        }
    }
}
