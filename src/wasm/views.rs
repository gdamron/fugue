//! `FugueEngine.takeViewsInto`: the views reading for a player's audio
//! thread, written as numbers into a buffer the player owns. Unlike
//! `takeLevels` and `getControl`, which build JSON strings and copy the
//! module id into wasm memory each call, a read allocates nothing: the
//! clock is named once, ahead, with `setViewsClock`.

use js_sys::Float64Array;
use wasm_bindgen::prelude::*;

use super::WasmFugueEngine;

/// Slots in the buffer `takeViewsInto` fills, in order.
const VIEWS_LEN: usize = 9;

#[wasm_bindgen(js_class = FugueEngine)]
impl WasmFugueEngine {
    #[wasm_bindgen(js_name = setViewsClock)]
    /// Names the clock whose beats `takeViewsInto` reads, or none. It copies
    /// the id, so call it when the clock changes (a load or an edit), never
    /// per read.
    pub fn set_views_clock(&mut self, clock: Option<String>) {
        self.views_clock = clock;
    }

    #[wasm_bindgen(js_name = viewsLength)]
    /// The length of the buffer `takeViewsInto` fills.
    pub fn views_length() -> usize {
        VIEWS_LEN
    }

    #[wasm_bindgen(js_name = takeViewsInto)]
    /// Takes the output's levels since the last read (the same stretch
    /// `takeLevels` measures, which then starts over) and the views clock's
    /// beats, written into `out` (length `viewsLength()`) as `[left_peak,
    /// right_peak, left_rms, right_rms, frames, left_overs, right_overs,
    /// non_finite_samples, beats]`. An absent RMS or beats count is NaN.
    /// Returns false, writing nothing, with no piece loaded. Allocates
    /// nothing on the wasm heap, so a player may call it from its audio
    /// thread; the copy out makes one short-lived typed-array view in JS.
    pub fn take_views_into(&mut self, out: &Float64Array) -> Result<bool, JsValue> {
        if out.length() as usize != VIEWS_LEN {
            return Err(JsValue::from_str(
                "views buffer length must be viewsLength()",
            ));
        }
        let Some(reading) = self.inner.take_view_reading(self.views_clock.as_deref()) else {
            return Ok(false);
        };
        let levels = reading.levels;
        let absent = |value: Option<f32>| value.map_or(f64::NAN, f64::from);
        let slots: [f64; VIEWS_LEN] = [
            f64::from(levels.left_peak),
            f64::from(levels.right_peak),
            absent(levels.left_rms),
            absent(levels.right_rms),
            levels.frames as f64,
            levels.left_overs as f64,
            levels.right_overs as f64,
            levels.non_finite_samples as f64,
            reading.beats.unwrap_or(f64::NAN),
        ];
        out.copy_from(&slots);
        Ok(true)
    }
}
