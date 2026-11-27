//! wasm-bindgen wrapper for the render engine.

use js_sys::Float32Array;
use wasm_bindgen::prelude::*;

use crate::{ControlValue, RenderEngine};

mod inspection;
mod notes;
mod preview;
mod revision;
mod views;

#[wasm_bindgen(js_name = FugueEngine)]
/// wasm-bindgen wrapper around `RenderEngine`.
///
/// In addition to offline rendering, this wrapper exposes orchestration APIs so
/// a surrounding JS host can inspect and mutate the graph and drive `code`
/// modules from JavaScript.
pub struct WasmFugueEngine {
    inner: RenderEngine,
    render_buffer: Vec<f32>,
    /// The clock whose beats `takeViewsInto` reads.
    views_clock: Option<String>,
    /// What `takeNoteWordsInto` drains into, allocated as notes are first
    /// followed.
    note_words: Vec<f64>,
}

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(constructor)]
    /// Creates a new engine for the given sample rate.
    pub fn new(sample_rate: u32) -> WasmFugueEngine {
        WasmFugueEngine {
            inner: RenderEngine::new(sample_rate),
            render_buffer: Vec::new(),
            views_clock: None,
            note_words: Vec::new(),
        }
    }

    #[wasm_bindgen(js_name = sampleRate)]
    /// Returns the configured sample rate.
    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    #[wasm_bindgen(js_name = provideSample)]
    /// Hands the engine the bytes of one file of a package (a WAV sample),
    /// named by the package id, its version and the file's path in it, and
    /// decodes it at the engine's rate. A document's package ref
    /// (`id@requirement:file`) then plays the highest provided version that
    /// meets the requirement; one with no provided match fails to load,
    /// naming the ref. Provide samples before loading the piece that plays
    /// them. Returns the source the file resolves to (`id@=version:file`).
    /// Provided samples are shared by every engine in this WebAssembly
    /// instance (an AudioWorklet's engine has an instance of its own) and
    /// stay for its lifetime. Decoding runs here, on the calling thread: in
    /// an AudioWorklet that is the render thread, so provide samples before
    /// the piece that plays them starts, not while something plays.
    pub fn provide_sample(
        &self,
        package: &str,
        version: &str,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<String, JsValue> {
        crate::modules::sample_loading::provide_package_sample(
            package,
            version,
            path,
            bytes,
            self.inner.sample_rate(),
        )
        .map_err(|err| JsValue::from_str(&err))
    }

    #[wasm_bindgen(js_name = loadInvention)]
    /// Loads invention JSON into the render engine.
    pub fn load_invention(&mut self, json: &str) -> Result<(), JsValue> {
        self.inner.load_json(json).map_err(to_js_error)
    }

    /// Resets engine state without reloading the invention definition.
    pub fn reset(&mut self) -> Result<(), JsValue> {
        self.inner.reset().map_err(to_js_error)
    }

    #[wasm_bindgen(js_name = setControlNumber)]
    /// Sets a numeric control value.
    pub fn set_control_number(
        &self,
        module_id: &str,
        key: &str,
        value: f32,
    ) -> Result<(), JsValue> {
        self.inner
            .set_control(module_id, key, ControlValue::Number(value))
            .map_err(to_js_error)
    }

    #[wasm_bindgen(js_name = setControlBool)]
    /// Sets a boolean control value.
    pub fn set_control_bool(&self, module_id: &str, key: &str, value: bool) -> Result<(), JsValue> {
        self.inner
            .set_control(module_id, key, ControlValue::Bool(value))
            .map_err(to_js_error)
    }

    #[wasm_bindgen(js_name = setControlString)]
    /// Sets a string control value.
    pub fn set_control_string(
        &self,
        module_id: &str,
        key: &str,
        value: &str,
    ) -> Result<(), JsValue> {
        self.inner
            .set_control(module_id, key, ControlValue::String(value.to_string()))
            .map_err(to_js_error)
    }

    #[wasm_bindgen(js_name = setControls)]
    /// Makes a JSON array of control writes (each `module_id`, `key`,
    /// `value`, and optionally `intent`, `source` and `priority`) in order,
    /// or none of them, as a daemon's `set_controls` does: a value in a
    /// loose form (number text, a choice in another case) is read as its
    /// control's kind, and a number outside the range its control holds is
    /// set to the nearer limit.
    ///
    /// Returns `{"controls": [...]}` as JSON text, an entry per write with
    /// the daemon's fields: `module_id`, `key`, `value` (read back),
    /// `state`, `requested`, `clamped` when it was clamped, and `reason`
    /// when it failed as it was made. A refusal throws the daemon's error
    /// as JSON text: `code` `invalid_control_write`, `message`, and `write`
    /// with the refused write's `index`, `module_id`, `key`, `reason` and
    /// `message`; nothing was written.
    pub fn set_controls(&self, writes_json: &str) -> Result<String, JsValue> {
        let writes: Vec<crate::ControlWrite> =
            serde_json::from_str(writes_json).map_err(|err| {
                rpc_error_value(&crate::rpc::RpcError::new(
                    crate::rpc::RpcErrorCode::InvalidRequest,
                    format!("invalid control writes JSON: {err}; nothing was applied"),
                ))
            })?;
        let controls = (self.inner.write_controls(&writes)).map_err(|err| rpc_error_value(&err))?;
        let reply = serde_json::json!({ "controls": controls });
        Ok(reply.to_string())
    }

    #[wasm_bindgen(js_name = status)]
    /// Returns runtime status as a JSON string.
    pub fn status(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner.status())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = listModules)]
    /// Returns the current module snapshot as a JSON string.
    pub fn list_modules(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner.list_modules())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = listConnections)]
    /// Returns the current connection snapshot as a JSON string.
    pub fn list_connections(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner.list_connections())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = listCodeModules)]
    /// Returns discovered `code` modules and their runtime config as JSON.
    pub fn list_code_modules(&self) -> Result<String, JsValue> {
        let modules = self.inner.list_code_modules().map_err(to_graph_error)?;
        serde_json::to_string(&modules).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = getCodeModuleConfig)]
    /// Returns one `code` module's runtime config as JSON.
    pub fn get_code_module_config(&self, id: &str) -> Result<String, JsValue> {
        let module = self.inner.get_code_module(id).map_err(to_graph_error)?;
        serde_json::to_string(&module).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = addModule)]
    /// Adds a module to the loaded graph.
    pub fn add_module(
        &self,
        id: &str,
        module_type: &str,
        config_json: Option<String>,
    ) -> Result<(), JsValue> {
        let config = parse_config_json(config_json)?;
        self.inner
            .add_module(id, module_type, &config)
            .map_err(to_graph_error)
    }

    #[wasm_bindgen(js_name = removeModule)]
    /// Removes a module from the loaded graph.
    pub fn remove_module(&self, id: &str) -> Result<(), JsValue> {
        self.inner.remove_module(id).map_err(to_graph_error)
    }

    #[wasm_bindgen(js_name = applyEdits)]
    /// Applies a JSON array of `apply_edits` operations (`add_module`,
    /// `remove_module`, `swap_module`, `connect`, `disconnect`,
    /// `set_control`) to the loaded graph as one change, or refuses them all
    /// with nothing changed.
    ///
    /// Returns the batch's report as JSON text (see [`RenderEditsReport`]):
    /// `controls_written`, `controls_failed` when a write failed as it was
    /// made, `configs`, each written module's config as the engine's
    /// document now holds it, for a host that keeps its own copy, and
    /// `swapped` when the batch swapped a module: each swap's edit index,
    /// module and the connections it removed, as the daemon reports them.
    ///
    /// A refusal throws the daemon's error as JSON text: `code`, `message`
    /// and, when one edit is to blame, `edit` with its `index`, `op` and
    /// `reason`. The edits build and install on the calling thread, between
    /// renders: on an AudioWorklet that is the audio thread, so a batch
    /// that builds heavy modules, or checks `set_control` writes on
    /// throwaway copies of their modules, can delay the next render.
    ///
    /// [`RenderEditsReport`]: crate::RenderEditsReport
    pub fn apply_edits(&self, edits_json: &str) -> Result<String, JsValue> {
        let edits: Vec<crate::rpc::StructuralEdit> =
            serde_json::from_str(edits_json).map_err(|err| {
                rpc_error_value(&crate::rpc::RpcError::new(
                    crate::rpc::RpcErrorCode::InvalidRequest,
                    format!("invalid edits JSON: {err}; nothing was applied"),
                ))
            })?;
        let report = (self.inner.apply_edits(&edits)).map_err(|err| rpc_error_value(&err))?;
        serde_json::to_string(&report).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = setCodeModuleStatus)]
    /// Updates the runtime status string for a `code` module.
    pub fn set_code_module_status(&self, id: &str, status: &str) -> Result<(), JsValue> {
        self.inner
            .set_code_module_status(id, status)
            .map_err(to_graph_error)
    }

    #[wasm_bindgen(js_name = setCodeModuleError)]
    /// Updates the last-error string for a `code` module.
    pub fn set_code_module_error(&self, id: &str, error: &str) -> Result<(), JsValue> {
        self.inner
            .set_code_module_error(id, error)
            .map_err(to_graph_error)
    }

    /// Connects two module ports.
    pub fn connect(
        &self,
        from: &str,
        from_port: &str,
        to: &str,
        to_port: &str,
    ) -> Result<(), JsValue> {
        self.inner
            .connect(from, from_port, to, to_port)
            .map_err(to_graph_error)
    }

    /// Disconnects two module ports.
    pub fn disconnect(
        &self,
        from: &str,
        from_port: &str,
        to: &str,
        to_port: &str,
    ) -> Result<(), JsValue> {
        self.inner
            .disconnect(from, from_port, to, to_port)
            .map_err(to_graph_error)
    }

    #[wasm_bindgen(js_name = listControls)]
    /// Returns control metadata as a JSON string.
    pub fn list_controls(&self, module_id: Option<String>) -> Result<String, JsValue> {
        let controls = self
            .inner
            .list_controls(module_id.as_deref())
            .map_err(to_graph_error)?;
        let payload: Vec<_> = controls
            .into_iter()
            .map(|(module_id, controls)| serde_json::json!({ "module_id": module_id, "controls": controls }))
            .collect();
        serde_json::to_string(&payload).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = getControl)]
    /// Returns a control value encoded as JSON.
    pub fn get_control_json(&self, module_id: &str, key: &str) -> Result<String, JsValue> {
        let value = self
            .inner
            .get_control(module_id, key)
            .map_err(to_js_error)?;
        serde_json::to_string(&value).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = renderInterleaved)]
    /// Renders a block of interleaved stereo samples.
    pub fn render_interleaved(&mut self, frame_count: usize) -> Result<Vec<f32>, JsValue> {
        let samples = frame_count
            .checked_mul(2)
            .ok_or_else(|| JsValue::from_str("frame count overflowed"))?;
        let mut output = vec![0.0f32; samples];
        self.inner
            .render_interleaved(&mut output)
            .map_err(to_js_error)?;
        Ok(output)
    }

    #[wasm_bindgen(js_name = takeLevels)]
    /// The output's levels since the last call (or since the piece loaded),
    /// as JSON: `left_peak` and `right_peak` (linear, read before the
    /// device clamp, so an over reads above 1.0), `left_rms` and `right_rms`
    /// (of the samples rendered, after the clamp; absent with no frames),
    /// `frames`, `left_overs` and `right_overs` (samples beyond ±1.0, which
    /// the clamp cut) and `non_finite_samples` (NaN or ±inf samples the
    /// sinks silenced; a sink rebuilt during the stretch takes its count
    /// with it). Call it between renders to measure a stretch: the next
    /// starts after it.
    pub fn take_levels(&mut self) -> Result<String, JsValue> {
        let levels = self
            .inner
            .take_levels()
            .ok_or_else(|| JsValue::from_str("no invention loaded"))?;
        serde_json::to_string(&levels).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = finishAudioFileSink)]
    /// Finalizes an in-memory `audio_file_sink` and returns JSON stats.
    pub fn finish_audio_file_sink(&self, module_id: &str) -> Result<String, JsValue> {
        let stats = self
            .inner
            .finish_audio_file_sink(module_id)
            .map_err(to_js_error)?;
        serde_json::to_string(&stats).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = getAudioFileSinkWav)]
    /// Returns finalized WAV bytes from an in-memory `audio_file_sink`.
    pub fn audio_file_sink_wav(&self, module_id: &str) -> Result<Vec<u8>, JsValue> {
        self.inner
            .audio_file_sink_wav_bytes(module_id)
            .map_err(to_js_error)
    }

    #[wasm_bindgen(js_name = renderInterleavedInto)]
    /// Renders interleaved stereo samples into a caller-provided buffer.
    ///
    /// The buffer length must be even and is interpreted as
    /// `[left0, right0, left1, right1, ...]`.
    pub fn render_interleaved_into(&mut self, output: &Float32Array) -> Result<usize, JsValue> {
        let samples = output.length() as usize;
        if !samples.is_multiple_of(2) {
            return Err(JsValue::from_str("output buffer length must be even"));
        }
        if self.render_buffer.len() != samples {
            self.render_buffer.resize(samples, 0.0);
        }

        let frames = self
            .inner
            .render_interleaved(&mut self.render_buffer)
            .map_err(to_js_error)?;
        output.copy_from(&self.render_buffer);
        Ok(frames)
    }
}

fn to_js_error(error: Box<dyn std::error::Error>) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn to_graph_error(error: crate::GraphCommandError) -> JsValue {
    JsValue::from_str(&error.to_string())
}

/// `error` as JSON text a host can parse, or its message should it not
/// serialize.
fn rpc_error_value(error: &crate::rpc::RpcError) -> JsValue {
    JsValue::from_str(&serde_json::to_string(error).unwrap_or_else(|_| error.message.clone()))
}

fn parse_config_json(config_json: Option<String>) -> Result<serde_json::Value, JsValue> {
    match config_json {
        None => Ok(serde_json::Value::Null),
        Some(config_json) => serde_json::from_str(&config_json)
            .map_err(|err| JsValue::from_str(&format!("invalid config JSON: {}", err))),
    }
}
