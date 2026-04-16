//! wasm-bindgen wrapper for the render engine.

use wasm_bindgen::prelude::*;

use crate::{ControlValue, RenderEngine};

#[wasm_bindgen(js_name = FugueEngine)]
pub struct WasmFugueEngine {
    inner: RenderEngine,
}

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(constructor)]
    pub fn new(sample_rate: u32) -> WasmFugueEngine {
        WasmFugueEngine {
            inner: RenderEngine::new(sample_rate),
        }
    }

    #[wasm_bindgen(js_name = sampleRate)]
    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    #[wasm_bindgen(js_name = loadInvention)]
    pub fn load_invention(&mut self, json: &str) -> Result<(), JsValue> {
        self.inner.load_json(json).map_err(to_js_error)
    }

    pub fn reset(&mut self) -> Result<(), JsValue> {
        self.inner.reset().map_err(to_js_error)
    }

    #[wasm_bindgen(js_name = setControlNumber)]
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
    pub fn set_control_bool(&self, module_id: &str, key: &str, value: bool) -> Result<(), JsValue> {
        self.inner
            .set_control(module_id, key, ControlValue::Bool(value))
            .map_err(to_js_error)
    }

    #[wasm_bindgen(js_name = setControlString)]
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

    #[wasm_bindgen(js_name = status)]
    pub fn status(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner.status())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = listModules)]
    pub fn list_modules(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner.list_modules())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = listConnections)]
    pub fn list_connections(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.inner.list_connections())
            .map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = addModule)]
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
    pub fn remove_module(&self, id: &str) -> Result<(), JsValue> {
        self.inner.remove_module(id).map_err(to_graph_error)
    }

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
    pub fn get_control_json(&self, module_id: &str, key: &str) -> Result<String, JsValue> {
        let value = self
            .inner
            .get_control(module_id, key)
            .map_err(to_js_error)?;
        serde_json::to_string(&value).map_err(|err| JsValue::from_str(&err.to_string()))
    }

    #[wasm_bindgen(js_name = renderInterleaved)]
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
}

fn to_js_error(error: Box<dyn std::error::Error>) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn to_graph_error(error: crate::GraphCommandError) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn parse_config_json(config_json: Option<String>) -> Result<serde_json::Value, JsValue> {
    match config_json {
        None => Ok(serde_json::Value::Null),
        Some(config_json) => serde_json::from_str(&config_json)
            .map_err(|err| JsValue::from_str(&format!("invalid config JSON: {}", err))),
    }
}
