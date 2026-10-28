//! Written numbers as exact as their declaration needs them.
//!
//! A front door that narrows a written number to an f32 before the
//! control's declaration reads it can misreport it: past 2^23 an f32 has
//! no fractions, so `8388608.5` and `16777214.5` arrive whole, and
//! `60.0000001` rounds to `60`. A whole-number control would then take a
//! fraction it refuses. A number its f32 would misreport like that keeps
//! its text instead, for the declaration to read exactly and narrow only
//! once it has accepted it; every other number narrows as it always did.

use serde::{Deserialize, Deserializer};

use crate::ControlValue;

/// Whether an f32 misreports a number as a whole number it is not: it
/// rounds a fraction to a whole number, or one whole number to another.
/// A whole number (`whole`, when the number is one) is compared exactly,
/// so one past 2^53 that its f64 `float` also rounds counts too.
fn misread_by_f32(whole: Option<i128>, float: f64) -> bool {
    let narrowed = float as f32;
    match whole {
        Some(whole) => narrowed as i128 != whole,
        None => narrowed.is_finite() && narrowed.fract() == 0.0 && f64::from(narrowed) != float,
    }
}

/// `number` as [`misread_by_f32`] reads it.
fn json_misread(number: &serde_json::Number) -> bool {
    let whole = number
        .as_i64()
        .map(i128::from)
        .or_else(|| number.as_u64().map(i128::from));
    misread_by_f32(whole, number.as_f64().unwrap_or(0.0))
}

/// Whether `value` is number text an f32 misreads, which a whole-number
/// control's declaration reads from its text (see the module docs).
pub(crate) fn keeps_text(value: &ControlValue) -> bool {
    match value {
        ControlValue::String(text) => {
            let text = text.trim();
            let whole = text.parse::<i128>().ok();
            text.parse::<f64>()
                .is_ok_and(|float| misread_by_f32(whole, float))
        }
        _ => false,
    }
}

/// A JSON number as a write carries it: an f32, or its text when an f32
/// misreads it. A number control reads either as the same f32.
pub(crate) fn json_number(number: &serde_json::Number) -> ControlValue {
    if json_misread(number) {
        ControlValue::String(number.to_string())
    } else {
        ControlValue::Number(number.as_f64().unwrap_or(0.0) as f32)
    }
}

/// A JSON scalar as a write carries it (see [`json_number`]), or `None`
/// for an array, object or null.
pub(crate) fn json_scalar(value: &serde_json::Value) -> Option<ControlValue> {
    match value {
        serde_json::Value::Number(number) => Some(json_number(number)),
        serde_json::Value::Bool(flag) => Some(ControlValue::Bool(*flag)),
        serde_json::Value::String(text) => Some(ControlValue::String(text.clone())),
        _ => None,
    }
}

impl ControlValue {
    /// A JSON scalar as a control write carries it, for a host that takes
    /// control values as JSON; `None` for an array, object or null.
    ///
    /// A number an f32 would misreport as a whole number it is not (a
    /// whole number past 2^24 an f32 cannot hold, or a fraction it rounds
    /// to one) keeps its exact text, so a whole-number control reads it
    /// exactly and refuses what it cannot hold rather than taking a
    /// rounded value. Every other number narrows to an f32, as `as f32`
    /// would; narrowing every number that way loses this precision before
    /// any control sees it.
    ///
    /// ```
    /// use fugue::ControlValue;
    /// use serde_json::json;
    ///
    /// let exact = |value| ControlValue::from_json(&value).unwrap();
    /// assert_eq!(exact(json!(16_777_217)), ControlValue::String("16777217".into()));
    /// assert_eq!(exact(json!(9_007_199_254_740_991_i64)), ControlValue::String("9007199254740991".into()));
    /// assert_eq!(exact(json!(i64::MAX)), ControlValue::String(i64::MAX.to_string()));
    /// assert_eq!(exact(json!(-i64::MAX)), ControlValue::String((-i64::MAX).to_string()));
    /// assert_eq!(exact(json!(60)), ControlValue::Number(60.0));
    /// assert_eq!(exact(json!(0.74)), ControlValue::Number(0.74));
    /// assert_eq!(exact(json!(true)), ControlValue::Bool(true));
    /// assert_eq!(ControlValue::from_json(&json!(null)), None);
    /// ```
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        json_scalar(value)
    }
}

/// Deserializes a written control value as [`ControlValue`] does, keeping
/// the text of a number an f32 misreads (see [`json_number`]).
pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<ControlValue, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    json_scalar(&value).ok_or_else(|| {
        serde::de::Error::custom("data did not match any variant of untagged enum ControlValue")
    })
}

#[cfg(test)]
mod tests;
