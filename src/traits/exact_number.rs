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

/// Whether an f32 misreports `number` as a whole number it is not: it
/// rounds a fraction to a whole number, or one whole number to another.
fn misread_by_f32(number: f64) -> bool {
    let narrowed = number as f32;
    narrowed.is_finite() && narrowed.fract() == 0.0 && f64::from(narrowed) != number
}

/// Whether `value` is number text an f32 misreads, which a whole-number
/// control's declaration reads from its text (see the module docs).
pub(crate) fn keeps_text(value: &ControlValue) -> bool {
    match value {
        ControlValue::String(text) => text.trim().parse::<f64>().is_ok_and(misread_by_f32),
        _ => false,
    }
}

/// A JSON number as a write carries it: an f32, or its text when an f32
/// misreads it. A number control reads either as the same f32.
pub(crate) fn json_number(number: &serde_json::Number) -> ControlValue {
    match number.as_f64() {
        Some(float) if misread_by_f32(float) => ControlValue::String(number.to_string()),
        float => ControlValue::Number(float.unwrap_or(0.0) as f32),
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
