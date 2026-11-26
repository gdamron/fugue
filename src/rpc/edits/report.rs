//! What a committed `ApplyEdits` batch reports.

use serde::{Deserialize, Serialize};

use crate::RuntimeConnectionInfo;

/// What a committed `ApplyEdits` batch did. Its size depends on the batch,
/// never on the size of the invention: every list holds at most one entry per
/// edit.
///
/// The response envelope's `revision` is the revision the batch committed at.
/// A field a newer daemon adds reads as its default from an older one.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct ApplyEditsReport {
    /// How many edits the batch carried; all of them were applied.
    pub edit_count: usize,
    /// Module ids added to the running graph.
    pub added: Vec<String>,
    /// Module ids removed from the running graph.
    pub removed: Vec<String>,
    /// Module ids that existed before the batch and were removed and added
    /// again in it. Each gets a fresh instance, even when its type and config
    /// are identical, so its state restarts.
    pub rebuilt: Vec<String>,
    /// Each distinct control the batch's `set_control` edits wrote, once, in
    /// first-written order. Only controls of modules that exist after the
    /// commit are listed: a write to a module a later edit removed or
    /// replaced is dropped with that module.
    pub controls_written: Vec<WrittenControl>,
    /// Control writes that passed validation but still failed when the
    /// commit wrote them (a sample that did not load, say). The batch is
    /// committed all the same: the module keeps the value it had, and the
    /// retained document records that value. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls_failed: Vec<ControlWriteFailure>,
    /// Each `swap_module` edit, in batch order, with the connections it
    /// removed. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swapped: Vec<SwappedModule>,
    pub connections_added: usize,
    /// Connections removed, against the piece before the batch: a swap's
    /// removals count; one the batch both made and removed, or one that went
    /// with a removed module, does not.
    pub connections_removed: usize,
    /// Modules the batch left untouched; they keep their phase and state.
    /// A control-only change counts as untouched: the value changes, the
    /// instance does not.
    pub untouched: usize,
}

/// One control a committed batch wrote, named structurally because a module
/// id may itself contain a `.`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct WrittenControl {
    pub module_id: String,
    pub key: String,
    /// Whether the number its last write gave lay outside the range the
    /// control holds, so the control was set to the nearer limit, as the
    /// written module's config now records it. Omitted when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clamped: bool,
}

/// A control write a committed batch could not make; see
/// [`ApplyEditsReport::controls_failed`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct ControlWriteFailure {
    /// The `set_control` edit whose value failed: the last edit in the batch
    /// that wrote this control.
    pub edit_index: usize,
    pub module_id: String,
    pub key: String,
    /// The module's reason, cut to at most 256 bytes on a UTF-8 character
    /// boundary.
    pub error: String,
}

/// What one `swap_module` edit did to the module's connections; see
/// [`ApplyEditsReport::swapped`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(default)]
pub struct SwappedModule {
    /// The `swap_module` edit's position in the batch.
    pub edit_index: usize,
    pub module_id: String,
    /// The connections touching the module whose port its new type lacks,
    /// as they were at this point in the batch, in the order the piece
    /// held them.
    pub connections_removed: Vec<RuntimeConnectionInfo>,
}

impl WrittenControl {
    pub fn new(module_id: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            module_id: module_id.into(),
            key: key.into(),
            clamped: false,
        }
    }

    /// This control, its last write `clamped` or not.
    pub fn clamped(self, clamped: bool) -> Self {
        Self { clamped, ..self }
    }
}
