//! `FugueEngine.advancesRevision` and `FugueEngine.checkRevision`: core's
//! revision rule for a browser host that keeps its own revision, so its
//! revision moves, and refuses a stale precondition, as a daemon's does.

use wasm_bindgen::prelude::*;

use super::{rpc_error_value, WasmFugueEngine};
use crate::rpc::{RpcCommand, RpcError, RpcErrorCode, RuntimeRevision};

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = advancesRevision)]
    /// Whether a host advances its revision once `command_json` (a daemon
    /// command as JSON, e.g. `{"command":"set_controls","writes":[...]}`)
    /// has run; `committed` is whether it made its change. A control write
    /// advances once when one of its writes authors (the default intent),
    /// never when all perform, and not when it was refused; a structural
    /// edit advances when it commits. For a `set_control`, the answer is
    /// also whether its write is authored: recorded in the document,
    /// unless it is an event control, which no document holds.
    ///
    /// A command that does not parse throws the daemon's error as JSON
    /// text, code `invalid_request`.
    pub fn advances_revision(command_json: &str, committed: bool) -> Result<bool, JsValue> {
        let command: RpcCommand = serde_json::from_str(command_json)
            .map_err(|err| invalid(format!("invalid command JSON: {err}")))?;
        Ok(command.advances_revision_after(committed))
    }

    #[wasm_bindgen(js_name = checkRevision)]
    /// Checks a call's `expected_revision` (`expected_json`, or `null` for
    /// none, which always passes) against `current_json`, the revision the
    /// host holds, each `{"session_id", "revision"}`. A stale or replaced
    /// one throws the daemon's error as JSON text: code
    /// `revision_conflict`, its `message`, and `conflict` with `expected`,
    /// `current` and `reason`; the host then writes nothing. One that does
    /// not parse throws code `invalid_request`.
    pub fn check_revision(expected_json: &str, current_json: &str) -> Result<(), JsValue> {
        let expected: Option<RuntimeRevision> = serde_json::from_str(expected_json)
            .map_err(|err| invalid(format!("invalid expected_revision: {err}; nothing changed")))?;
        let current: RuntimeRevision = serde_json::from_str(current_json)
            .map_err(|err| invalid(format!("invalid current revision: {err}")))?;
        (current.check(expected.as_ref()))
            .map_err(|conflict| rpc_error_value(&RpcError::revision_conflict(conflict)))
    }
}

fn invalid(message: String) -> JsValue {
    rpc_error_value(&RpcError::new(RpcErrorCode::InvalidRequest, message))
}
