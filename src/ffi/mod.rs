//! Native C ABI for the render engine.

#![cfg(feature = "ffi")]

use std::ffi::{c_char, CStr};

use crate::{ControlValue, RenderEngine};

/// Opaque engine handle for C consumers.
pub struct FugueEngine {
    inner: RenderEngine,
    last_error: Vec<u8>,
}

impl FugueEngine {
    fn new(sample_rate: u32) -> Self {
        Self {
            inner: RenderEngine::new(sample_rate),
            last_error: vec![0],
        }
    }

    fn clear_error(&mut self) {
        self.last_error.clear();
        self.last_error.push(0);
    }

    fn set_error(&mut self, message: impl AsRef<str>) {
        let mut bytes: Vec<u8> = message
            .as_ref()
            .bytes()
            .map(|byte| if byte == 0 { b' ' } else { byte })
            .collect();
        bytes.push(0);
        self.last_error = bytes;
    }
}

unsafe fn engine_mut<'a>(engine: *mut FugueEngine) -> Result<&'a mut FugueEngine, ()> {
    engine.as_mut().ok_or(())
}

unsafe fn engine_ref<'a>(engine: *const FugueEngine) -> Result<&'a FugueEngine, ()> {
    engine.as_ref().ok_or(())
}

unsafe fn read_utf8<'a>(data: *const u8, len: usize) -> Result<&'a str, &'static str> {
    if len == 0 {
        return Ok("");
    }
    if data.is_null() {
        return Err("input buffer was null");
    }
    let bytes = std::slice::from_raw_parts(data, len);
    std::str::from_utf8(bytes).map_err(|_| "input buffer was not valid UTF-8")
}

unsafe fn read_cstr<'a>(value: *const c_char, field: &'static str) -> Result<&'a str, String> {
    if value.is_null() {
        return Err(format!("{} was null", field));
    }
    CStr::from_ptr(value)
        .to_str()
        .map_err(|_| format!("{} was not valid UTF-8", field))
}

#[no_mangle]
pub extern "C" fn fugue_engine_new(sample_rate: u32) -> *mut FugueEngine {
    Box::into_raw(Box::new(FugueEngine::new(sample_rate)))
}

#[no_mangle]
pub unsafe extern "C" fn fugue_engine_free(engine: *mut FugueEngine) {
    if !engine.is_null() {
        drop(Box::from_raw(engine));
    }
}

#[no_mangle]
pub unsafe extern "C" fn fugue_engine_load_json(
    engine: *mut FugueEngine,
    json: *const u8,
    json_len: usize,
) -> i32 {
    let Ok(engine) = engine_mut(engine) else {
        return 0;
    };

    let result = match read_utf8(json, json_len) {
        Ok(json) => engine.inner.load_json(json),
        Err(message) => Err(message.into()),
    };

    match result {
        Ok(()) => {
            engine.clear_error();
            1
        }
        Err(err) => {
            engine.set_error(err.to_string());
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn fugue_engine_reset(engine: *mut FugueEngine) -> i32 {
    let Ok(engine) = engine_mut(engine) else {
        return 0;
    };

    match engine.inner.reset() {
        Ok(()) => {
            engine.clear_error();
            1
        }
        Err(err) => {
            engine.set_error(err.to_string());
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn fugue_engine_render_interleaved(
    engine: *mut FugueEngine,
    output: *mut f32,
    frame_count: usize,
) -> usize {
    let Ok(engine) = engine_mut(engine) else {
        return 0;
    };

    if frame_count == 0 {
        engine.clear_error();
        return 0;
    }
    if output.is_null() {
        engine.set_error("output buffer was null");
        return 0;
    }

    let samples = match frame_count.checked_mul(2) {
        Some(samples) => samples,
        None => {
            engine.set_error("frame count overflowed");
            return 0;
        }
    };
    let buffer = std::slice::from_raw_parts_mut(output, samples);

    match engine.inner.render_interleaved(buffer) {
        Ok(frames) => {
            engine.clear_error();
            frames
        }
        Err(err) => {
            engine.set_error(err.to_string());
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn fugue_engine_set_control_number(
    engine: *mut FugueEngine,
    module_id: *const c_char,
    key: *const c_char,
    value: f32,
) -> i32 {
    set_control(engine, module_id, key, ControlValue::Number(value))
}

#[no_mangle]
pub unsafe extern "C" fn fugue_engine_set_control_bool(
    engine: *mut FugueEngine,
    module_id: *const c_char,
    key: *const c_char,
    value: bool,
) -> i32 {
    set_control(engine, module_id, key, ControlValue::Bool(value))
}

#[no_mangle]
pub unsafe extern "C" fn fugue_engine_set_control_string(
    engine: *mut FugueEngine,
    module_id: *const c_char,
    key: *const c_char,
    value: *const c_char,
) -> i32 {
    let Ok(engine) = engine_mut(engine) else {
        return 0;
    };

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let module_id = read_cstr(module_id, "module_id")?;
        let key = read_cstr(key, "key")?;
        let value = read_cstr(value, "value")?;
        engine
            .inner
            .set_control(module_id, key, ControlValue::String(value.to_string()))
    })();

    match result {
        Ok(()) => {
            engine.clear_error();
            1
        }
        Err(err) => {
            engine.set_error(err.to_string());
            0
        }
    }
}

unsafe fn set_control(
    engine: *mut FugueEngine,
    module_id: *const c_char,
    key: *const c_char,
    value: ControlValue,
) -> i32 {
    let Ok(engine) = engine_mut(engine) else {
        return 0;
    };

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let module_id = read_cstr(module_id, "module_id")?;
        let key = read_cstr(key, "key")?;
        engine.inner.set_control(module_id, key, value)
    })();

    match result {
        Ok(()) => {
            engine.clear_error();
            1
        }
        Err(err) => {
            engine.set_error(err.to_string());
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn fugue_engine_last_error(engine: *const FugueEngine) -> *const c_char {
    match engine_ref(engine) {
        Ok(engine) if engine.last_error.len() > 1 => engine.last_error.as_ptr() as *const c_char,
        _ => std::ptr::null(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn fugue_engine_clear_error(engine: *mut FugueEngine) {
    if let Ok(engine) = engine_mut(engine) {
        engine.clear_error();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    const SIMPLE_INVENTION: &str = r#"{
        "version": "1.0.0",
        "title": "ffi-test",
        "modules": [
            { "id": "osc", "type": "oscillator", "config": { "waveform": "sine", "frequency": 440.0 } },
            { "id": "vca", "type": "vca", "config": { "level": 0.0 } },
            { "id": "dac", "type": "dac", "config": { "soft_clip": false } }
        ],
        "connections": [
            { "from": "osc", "from_port": "audio", "to": "vca", "to_port": "audio" },
            { "from": "vca", "from_port": "audio", "to": "dac", "to_port": "audio" }
        ]
    }"#;

    #[test]
    fn ffi_engine_loads_and_renders() {
        let engine = fugue_engine_new(48_000);
        let module = CString::new("vca").unwrap();
        let key = CString::new("cv").unwrap();
        let mut output = [0.0f32; 16];

        unsafe {
            assert_eq!(
                fugue_engine_load_json(engine, SIMPLE_INVENTION.as_ptr(), SIMPLE_INVENTION.len()),
                1
            );
            assert_eq!(
                fugue_engine_set_control_number(engine, module.as_ptr(), key.as_ptr(), 0.5),
                1
            );
            assert_eq!(
                fugue_engine_render_interleaved(engine, output.as_mut_ptr(), 8),
                8
            );
            assert!(output.iter().any(|sample| sample.abs() > 0.0));
            fugue_engine_free(engine);
        }
    }
}
