//! `FugueEngine.advancesRevision`, `FugueEngine.checkRevision` and
//! `documentWithEpoch`: core's revision rule for a browser host that keeps
//! its own revision, so its revision moves, it refuses a stale
//! precondition, and its document records a write, as a daemon's does.

use wasm_bindgen::prelude::*;

use super::{rpc_error_value, WasmFugueEngine};
use crate::rpc::{EditTiming, RpcCommand, RpcError, RpcErrorCode, RuntimeRevision};
use crate::ControlWrite;

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = advancesRevision)]
    /// Whether a host advances its revision once a call has run, by the
    /// daemon's rule: `command` is `set_control`, `set_controls` or
    /// `apply_edits`, `writes_json` the call's control writes as JSON (`[]`
    /// for an edit batch), `committed` whether it made its change, and
    /// `timing_json` an edit batch's `{"when", "ttl_ms"}` when it is timed
    /// (`null` or omitted when it is not). A control write advances once
    /// when one of its writes authors (the default intent), never when all
    /// perform, and not when it was refused; an untimed edit batch advances
    /// when it commits, and a timed one as it is adopted, not as it is
    /// queued. Which writes the document recorded is `documentWithEpoch`'s
    /// to say.
    ///
    /// It takes the call's parts rather than a whole daemon command, so the
    /// engine carries no parser for commands it never runs. Another command,
    /// parts that do not fit the call (see [`RpcCommand::revision_call`]),
    /// or JSON that does not parse throw the daemon's error as JSON text,
    /// code `invalid_request`.
    pub fn advances_revision(
        command: &str,
        writes_json: &str,
        committed: bool,
        timing_json: Option<String>,
    ) -> Result<bool, JsValue> {
        let writes: Vec<ControlWrite> = serde_json::from_str(writes_json)
            .map_err(|err| invalid(format!("invalid control writes JSON: {err}")))?;
        let timing: Option<EditTiming> = match timing_json.as_deref() {
            Some(json) => serde_json::from_str(json)
                .map_err(|err| invalid(format!("invalid edit timing JSON: {err}")))?,
            None => None,
        };
        let command = RpcCommand::revision_call(command, writes, timing.unwrap_or_default())
            .map_err(|err| rpc_error_value(&err))?;
        Ok(command.advances_revision_after(committed))
    }

    #[wasm_bindgen(js_name = documentWithEpoch)]
    /// The retained document and its authored epoch, read together, as
    /// JSON text: `{"document": ..., "authored_epoch": n}`, the document
    /// `null` before one is loaded. The epoch moves once for each authored
    /// write the document recorded, so a host that reads it around a call
    /// knows whether the call recorded any, and the document holds what it
    /// recorded: a performed write, a write to an event control and one
    /// that failed as it was made record nothing.
    pub fn document_with_epoch(&self) -> String {
        let (document, epoch) = self.inner.document_with_epoch();
        serde_json::json!({ "document": document, "authored_epoch": epoch }).to_string()
    }

    #[wasm_bindgen(js_name = checkRevision)]
    /// Checks a call's `expected_revision` (`expected_json`, or the text
    /// `null` for none, which always passes; `undefined` is not JSON and
    /// throws `invalid_request`) against `current_json`, the revision the
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
