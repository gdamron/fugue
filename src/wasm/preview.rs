//! `FugueEngine.previewNotes`: the notes a document will play, worked out in
//! slices on the calling thread so the page stays responsive.

use js_sys::{Function, Promise};
use wasm_bindgen::prelude::*;

use super::WasmFugueEngine;
use crate::{NotePreview, PREVIEW_SAMPLE_RATE};

/// Milliseconds of work in one slice before yielding to the event loop:
/// far under the 50 ms a browser counts as a long task, with room for one
/// stretch of frames run past the budget on a slow device.
const SLICE_MS: f64 = 8.0;

/// Frames run between two looks at the clock (`Date.now()`).
const SLICE_FRAMES: u64 = 1024;

#[wasm_bindgen]
extern "C" {
    /// The global `setTimeout`: a page's, a worker's or Node's.
    #[wasm_bindgen(js_name = setTimeout)]
    fn set_timeout(callback: &JsValue, delay: i32) -> JsValue;
}

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = previewNotes)]
    /// Lists the notes `json` (an invention document) plays over a range of
    /// bars, as `request` (JSON) asks, without playing it or touching the
    /// engine's own piece. Resolves to the answer as JSON, or rejects with
    /// a string (see `docs/preview-notes.md`). Counts at
    /// `PREVIEW_SAMPLE_RATE` whatever the engine's rate, so it lists the
    /// notes a native preview lists. Works in slices that yield with
    /// `setTimeout(0)`: call it from a page or a worker, not an AudioWorklet.
    pub fn preview_notes(&self, json: String, request: String) -> Promise {
        let sample_rate = PREVIEW_SAMPLE_RATE;
        Promise::new(
            &mut |resolve, reject| match NotePreview::new(&json, &request, sample_rate) {
                Err(error) => {
                    let _ = reject.call1(&JsValue::NULL, &JsValue::from_str(&error));
                }
                Ok(preview) => schedule(preview, resolve, reject),
            },
        )
    }
}

/// Runs the next slice once the event loop has had its turn.
fn schedule(preview: NotePreview, resolve: Function, reject: Function) {
    let slice = Closure::once_into_js(move || work(preview, resolve, reject));
    set_timeout(&slice, 0);
}

/// One slice: runs the preview for about [`SLICE_MS`], then settles the
/// promise or schedules the next slice.
fn work(mut preview: NotePreview, resolve: Function, reject: Function) {
    let started = js_sys::Date::now();
    loop {
        if preview.advance(SLICE_FRAMES) {
            let _ = match preview.answer() {
                Ok(answer) => resolve.call1(&JsValue::NULL, &JsValue::from_str(&answer)),
                Err(error) => reject.call1(&JsValue::NULL, &JsValue::from_str(&error)),
            };
            return;
        }
        if js_sys::Date::now() - started >= SLICE_MS {
            return schedule(preview, resolve, reject);
        }
    }
}
