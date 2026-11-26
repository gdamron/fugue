//! Why a batch of control writes was refused.

use serde::{Deserialize, Serialize};

/// The write that refused a `SetControl` or `SetControls` batch, and why.
/// Nothing in the batch was written: a batch is checked whole before any
/// write is made.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct ControlWriteRefusal {
    /// The refused write's position in the batch, from 0.
    pub index: usize,
    pub module_id: String,
    pub key: String,
    pub reason: ControlWriteReason,
    /// Why, as the error's message gives it without the index.
    pub message: String,
}

/// Why a control write is refused.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ControlWriteReason {
    /// The write names a module the piece does not have.
    UnknownModule,
    /// The write names a key its module does not expose as a control.
    UnknownControl,
    /// The value is refused: it cannot be read as the control's kind (text
    /// that is no number for a number), it is not a finite number, or the
    /// module refuses it as its own setter would (a read-only control, an
    /// option the control does not offer, unparsable JSON).
    #[default]
    InvalidControlValue,
    /// The write's `when` or `ttl_ms` cannot be met as asked: a `ttl_ms`
    /// without a `when`, or a timed write to a control written at once (a
    /// part of another control, or undeclared host state).
    InvalidTiming,
}
