//! Serde adapters for the whole-number fields of structured config
//! (schedule entries, tempo maps), so they read a number as the config
//! reader does: a JSON integer, or a float that is exactly whole (`8.0`
//! reads as 8). A fractional or out-of-range value is refused.

use serde::de::Error;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::{whole_number, ConfigInt};

/// `deserialize_with` for a required whole-number field.
pub(crate) fn whole<'de, D: Deserializer<'de>, T: ConfigInt>(
    deserializer: D,
) -> Result<T, D::Error> {
    let value = Value::deserialize(deserializer)?;
    whole_number(&value).map_err(D::Error::custom)
}

/// `deserialize_with` for an optional whole-number field (with
/// `#[serde(default)]`); `null` reads as absent.
pub(crate) fn optional_whole<'de, D: Deserializer<'de>, T: ConfigInt>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(value) => whole_number(&value).map(Some).map_err(D::Error::custom),
    }
}
