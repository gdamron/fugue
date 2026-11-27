//! What differs between the piece as last loaded or saved and the piece now
//! (`docs/authored-inspection.md`): one comparison for every host.
//!
//! The document changes come first: modules removed, then each module in
//! document order, changed (each config key with its saved and current
//! value) or added; connections removed and added; then the piece's other
//! fields (title, description, each development and asset by name). Fields
//! and keys come in name order, the saved piece's first and then those only
//! the current one has. Then, apart, the values that sound but that
//! the document does not hold because they were only performed: a save
//! would not keep them.

mod compare;
#[cfg(test)]
mod tests;

use super::{RpcError, RpcErrorCode};
use crate::Invention;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A control's value as it sounds now, which a host reads off the audio
/// thread: the input to [`PerformedValue`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SoundingValue {
    pub module_id: String,
    pub key: String,
    pub value: Value,
}

/// One difference in the document. A value over 4 KiB is left out and
/// `omitted` is set: read it with `inspect_invention`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum DiffEntry {
    /// A module the saved piece did not have.
    ModuleAdded {
        id: String,
        #[serde(rename = "type")]
        module_type: String,
    },
    /// A module the saved piece had.
    ModuleRemoved {
        id: String,
        #[serde(rename = "type")]
        module_type: String,
    },
    /// One config key of a module in both; a side without the key has no value.
    ConfigChanged {
        module_id: String,
        key: String,
        #[serde(flatten)]
        values: Values,
    },
    /// A module field other than its config (its type, its name).
    ModuleChanged {
        id: String,
        field: String,
        #[serde(flatten)]
        values: Values,
    },
    /// A connection the saved piece did not have.
    ConnectionAdded { connection: Value },
    /// A connection the saved piece had.
    ConnectionRemoved { connection: Value },
    /// A field of the piece: title, description, or one development or
    /// asset, which `name` names.
    PieceChanged {
        field: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(flatten)]
        values: Values,
    },
}

/// A changed value as saved and now; a side that holds nothing has none.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct Values {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<Value>,
    /// A value too large to carry was left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub omitted: bool,
}

/// A control that sounds other than the document holds it, because the
/// value was only performed. `document` is absent where the document holds
/// nothing for the control (it plays its default there).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct PerformedValue {
    pub module_id: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document: Option<Value>,
    pub sounding: Value,
}

/// Every difference between two documents, in the order a reader lists them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Differences {
    /// The document changes.
    pub changes: Vec<DiffEntry>,
    /// The values a save would not keep. This is the one place that decides
    /// whether performed values are shown: leave it empty to show none.
    pub performed: Vec<PerformedValue>,
}

/// What differs from `baseline` to `current`, and which of the `sounding`
/// values (each control written as a performance, as it sounds now) the
/// current document does not hold.
pub fn differences(
    baseline: &Invention,
    current: &Invention,
    sounding: &[SoundingValue],
) -> Result<Differences, RpcError> {
    let (saved, current) = (to_value(baseline)?, to_value(current)?);
    Ok(Differences {
        changes: compare::changes(&saved, &current),
        performed: compare::performed(&current, sounding),
    })
}

fn to_value(invention: &Invention) -> Result<Value, RpcError> {
    serde_json::to_value(invention)
        .map_err(|e| RpcError::new(RpcErrorCode::Internal, e.to_string()))
}
