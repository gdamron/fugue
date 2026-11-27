//! `FugueEngine.advancesRevision`, `FugueEngine.checkRevision` and
//! `documentWithEpoch`: core's revision rule for a browser host that keeps
//! its own revision, so its revision moves, it refuses a stale
//! precondition, and its document records a write, as a daemon's does.

use wasm_bindgen::prelude::*;

use super::{rpc_error_value, WasmFugueEngine};
use crate::rpc::{RpcCommand, RpcError, RpcErrorCode, RuntimeRevision};
use crate::ControlWrite;

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = advancesRevision)]
    /// Whether a host advances its revision once a call has run, by the
    /// daemon's rule: `command` is `set_control`, `set_controls` or
    /// `apply_edits` (an untimed batch), `writes_json` the call's control
    /// writes as JSON (`[]` for an edit batch), and `committed` whether it
    /// made its change. A control write advances once when one of its
    /// writes authors (the default intent), never when all perform, and not
    /// when it was refused; an edit batch advances when it commits. Which
    /// writes the document recorded is `documentWithEpoch`'s to say.
    ///
    /// It takes the call's parts rather than a whole daemon command, so the
    /// engine carries no parser for commands it never runs. Another command,
    /// or writes that do not parse, throw the daemon's error as JSON text,
    /// code `invalid_request`.
    pub fn advances_revision(
        command: &str,
        writes_json: &str,
        committed: bool,
    ) -> Result<bool, JsValue> {
        let writes: Vec<ControlWrite> = serde_json::from_str(writes_json)
            .map_err(|err| invalid(format!("invalid control writes JSON: {err}")))?;
        let command = match (command, writes.as_slice()) {
            ("set_control", [write]) => RpcCommand::SetControl {
                module_id: write.module_id.clone(),
                key: write.key.clone(),
                value: write.value.clone(),
                intent: write.intent,
                source: write.source,
                priority: write.priority,
                timing: write.timing.clone(),
            },
            ("set_control", _) => return Err(invalid("a set_control makes one write".to_string())),
            ("set_controls", _) => RpcCommand::SetControls { writes },
            ("apply_edits", _) => RpcCommand::ApplyEdits {
                edits: Vec::new(),
                timing: Default::default(),
            },
            (other, _) => return Err(invalid(format!("no revision rule for `{other}` here"))),
        };
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
