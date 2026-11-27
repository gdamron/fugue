//! `FugueEngine.diffInventions`: the daemon's `GetDiff` over documents the
//! page holds (`docs/authored-inspection.md`).

use wasm_bindgen::prelude::*;

use super::{rpc_error_value, WasmFugueEngine};

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = diffInventions)]
    /// One page of what differs between `baseline` (invention JSON, the
    /// piece as last loaded or saved; `kind` is `"loaded"` or `"saved"`)
    /// and `current` (invention JSON), then the values in `sounding` (JSON
    /// `[{module_id, key, value}]`, the controls written as performances,
    /// as they sound) that `current` does not hold. Stamped with `revision`
    /// (JSON `{session_id, revision}`); `query` is JSON `{limit, cursor}`,
    /// or `null` for the first page. Returns the daemon's `Diff` page as
    /// compact JSON text. A refusal throws the daemon's error as JSON text:
    /// `code`, `message` and, for a stale cursor, `conflict`. Reads no
    /// engine: the documents are the caller's.
    pub fn diff_inventions(
        baseline: &str,
        kind: &str,
        current: &str,
        sounding: &str,
        revision: &str,
        query: &str,
    ) -> Result<String, JsValue> {
        let texts = crate::rpc::DiffTexts {
            baseline,
            kind,
            current,
            sounding,
            revision,
            query,
        };
        crate::rpc::diff_documents(&texts).map_err(|error| rpc_error_value(&error))
    }
}
