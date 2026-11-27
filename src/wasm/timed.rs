//! `FugueEngine.takeSettledWrites`: what became of the timed control writes
//! `setControls` queued, as the daemon's `ControlRequestSettled` events
//! report it. Its presence tells a host this engine takes `when`.

use wasm_bindgen::prelude::*;

use super::WasmFugueEngine;

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = takeSettledWrites)]
    /// Every timed control write that settled since the last call, oldest
    /// first, as a JSON array of the daemon's `ControlRequestSettled`
    /// fields: `request`, `module_id`, `key`, `state` (`applied`,
    /// `refused`, `overridden`...), `landing` (the sample it applied on,
    /// and the one it was due on when it applied late) and `reason` when
    /// it did not apply. Call it between renders, never inside one.
    pub fn take_settled_writes(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner.take_settled_writes())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }
}
