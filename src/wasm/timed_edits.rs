//! `FugueEngine.applyEditsAt`, `requestStatus` and `takeRequestChanges`:
//! an edit batch landing on a sample or a beat, followed as the daemon's
//! `GetRequest` follows one. Their presence tells a host this engine
//! times edit batches.

use wasm_bindgen::prelude::*;

use super::{rpc_error_value, WasmFugueEngine};
use crate::rpc::{EditWhen, RpcError, RpcErrorCode, StructuralEdit};

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = applyEditsAt)]
    /// [`applyEdits`](Self::apply_edits), landing at `when_json`, the
    /// daemon's `when` as JSON text (`at_sample`, `after_samples`,
    /// `after_beats`, `next_beat` or `next_bar`), rather than at once,
    /// unless `ttl_ms` milliseconds pass first. Returns the queued
    /// request's status as JSON text, as the daemon's `GetRequest` gives
    /// it: `id`, `state` (`queued`), `landing` and `reason`.
    ///
    /// Throws the daemon's error as JSON text when it refuses the batch, as
    /// an untimed one is refused, and also when it changes no structure,
    /// adds or removes a code or agent module, names a time that can never
    /// come, or another timed edit waits; a `set_control` in a timed batch
    /// is refused. Nothing is queued then.
    pub fn apply_edits_at(
        &self,
        edits_json: &str,
        when_json: &str,
        ttl_ms: Option<f64>,
    ) -> Result<String, JsValue> {
        let edits: Vec<StructuralEdit> = parse(edits_json, "edits")?;
        let when: EditWhen = parse(when_json, "when")?;
        let ttl_ms = ttl_ms.map(whole_ms).transpose()?;
        let status = (self.inner.apply_edits_at(&edits, &when, ttl_ms))
            .map_err(|err| rpc_error_value(&err))?;
        serde_json::to_string(&status).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = requestStatus)]
    /// Where timed request `id` stands, as JSON text with the daemon's
    /// `GetRequest` fields: `state` (`queued`, `landed`, `adopted`,
    /// `refused`, `expired` or `cancelled`), `landing` (the sample it
    /// landed on, and its beat), `reason`, and the modules it `built` and
    /// `removed` once adopted. `undefined` once the engine no longer
    /// remembers it: it keeps the latest few dozen of the invention
    /// loaded, and ids start again with each load. Call it between
    /// renders, never inside one.
    pub fn request_status(&self, id: &str) -> Result<Option<String>, JsValue> {
        let status = self.inner.request_status(id);
        let json = status
            .map(|status| serde_json::to_string(&status))
            .transpose();
        json.map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = takeRequestChanges)]
    /// Every timed request whose status changed since the last call, at its
    /// latest, as a JSON array of [`requestStatus`](Self::request_status)
    /// entries, in the order they last changed; a replaced invention's
    /// first, cancelled as it was replaced. Call it between renders.
    pub fn take_request_changes(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner.take_request_changes())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }
}

/// `json`, the argument `what`, read, or the daemon's `invalid_request`.
fn parse<T: serde::de::DeserializeOwned>(json: &str, what: &str) -> Result<T, JsValue> {
    serde_json::from_str(json).map_err(|err| {
        rpc_error_value(&RpcError::new(
            RpcErrorCode::InvalidRequest,
            format!("invalid {what} JSON: {err}; nothing was applied"),
        ))
    })
}

/// `ms`, a ttl, as the daemon reads one: a whole number of milliseconds,
/// 0 or more, or its `invalid_request`.
fn whole_ms(ms: f64) -> Result<u64, JsValue> {
    if ms.is_finite() && ms >= 0.0 && ms.fract() == 0.0 && ms <= u64::MAX as f64 {
        return Ok(ms as u64);
    }
    Err(rpc_error_value(&RpcError::new(
        RpcErrorCode::InvalidRequest,
        format!("invalid ttl_ms {ms}: expected a whole number of milliseconds, 0 or more; nothing was applied"),
    )))
}
