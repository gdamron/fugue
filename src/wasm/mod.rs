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
    pub fn set_control_bool(
        &self,
        module_id: &str,
        key: &str,
        value: bool,
    ) -> Result<(), JsValue> {
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
