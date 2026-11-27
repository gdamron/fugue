//! `FugueEngine.inspectInvention`: the daemon's bounded authored inspection
//! (`docs/authored-inspection.md`) of a document the page holds.

use wasm_bindgen::prelude::*;

use super::{rpc_error_value, WasmFugueEngine};

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = inspectInvention)]
    /// Reads one page of `document` (invention JSON) as `query` (JSON: the
    /// daemon's `selection`, `limit` and `cursor`) asks, stamped with
    /// `revision` (JSON `{session_id, revision}`, the host's own count).
    /// Returns the page as compact JSON text, the daemon's page with a null
    /// `source_path`. `expected_revision` (JSON), when given and not
    /// `revision`, is refused before anything is read. A refusal throws the
    /// daemon's error as JSON text: `code`, `message` and, for a revision
    /// conflict, `conflict`. Reads no engine: the document is the caller's.
    pub fn inspect_invention(
        document: &str,
        revision: &str,
        query: &str,
        expected_revision: Option<String>,
    ) -> Result<String, JsValue> {
        crate::rpc::inspect_document(document, revision, query, expected_revision.as_deref())
            .map_err(|error| rpc_error_value(&error))
    }
}
