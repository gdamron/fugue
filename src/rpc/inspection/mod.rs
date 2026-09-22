//! Bounded, revision-stamped views of retained authored declarations.

mod selection;
#[cfg(test)]
mod tests;

use super::{
    AuthoredSnapshot, ConflictReason, RevisionConflict, RpcError, RpcErrorCode, RuntimeRevision,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Compact JSON payload budget, before RPC or MCP string escaping.
pub const MAX_INSPECTION_BYTES: usize = 16 * 1024;
/// Maximum records per page; byte limits may produce a shorter page.
pub const MAX_INSPECTION_ENTRIES: usize = 100;
/// Individual values above this budget become explicit drill-down records.
pub const MAX_INSPECTION_VALUE_BYTES: usize = 4096;

/// A location in the authored document, never a flattened runtime module path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InspectionSelection {
    /// Inventory of an invention or inline development. Scope is its JSON pointer.
    Overview {
        #[serde(default)]
        scope: String,
    },
    /// Exact module declaration, incident wiring, peers and declared dependencies.
    Module {
        #[serde(default)]
        scope: String,
        id: String,
    },
    /// Development declaration, local instances/wiring, and inline inventory.
    Development {
        #[serde(default)]
        scope: String,
        name: String,
    },
    /// Immediate children of an authored JSON value; scalars return themselves.
    Value { pointer: String },
}

/// Continuation bound to both a selection and the authored revision that produced it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct InspectionCursor {
    /// Session identity and authored revision of this read.
    pub revision: RuntimeRevision,
    /// Authored selection used to enumerate this page.
    pub selection: InspectionSelection,
    /// Zero-based record offset within this selection.
    pub offset: usize,
}

/// One bounded inspection. Use the request envelope's expected_revision across selections.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InspectionQuery {
    /// Authored selection used to enumerate this page.
    pub selection: InspectionSelection,
    /// Defaults to 50; must be between 1 and 100.
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    /// Returned continuation; omit on the first read.
    pub cursor: Option<InspectionCursor>,
}

fn default_limit() -> usize {
    50
}

/// Whether the value is faithful, deliberately summarized, or too large to include.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum InspectionCoverage {
    /// Exact authored JSON value.
    Complete,
    /// Deliberately reduced inventory or peer information.
    Summary,
    /// Value exceeds the per-entry budget; inspect its pointer.
    Omitted,
    /// UTF-8 byte range of a string; concatenate fragments in range order.
    Fragment,
}

/// A record with an exact follow-up pointer into the original authored document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct InspectionEntry {
    /// RFC 6901 pointer into the retained document.
    pub pointer: String,
    /// Describes what is and is not included at this pointer.
    pub coverage: InspectionCoverage,
    /// Missing only for omitted values. JSON null remains a complete value.
    #[serde(
        default,
        deserialize_with = "present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub value: Option<Value>,
    /// Present for string fragments: half-open UTF-8 byte offsets in the original.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub string_range: Option<[usize; 2]>,
}

/// A page is complete only for its declared selection when no continuation or
/// non-complete entries remain. It is never a replacement invention document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct InspectionPage {
    /// Session identity and authored revision of this read.
    pub revision: RuntimeRevision,
    /// Original daemon source context, independent of client storage.
    pub source_path: Option<String>,
    /// Authored selection used to enumerate this page.
    pub selection: InspectionSelection,
    /// Number of records across all pages for this selection.
    pub total_entries: usize,
    /// Zero-based record offset within this selection.
    pub offset: usize,
    /// Records fitting both the requested count and byte budgets.
    pub entries: Vec<InspectionEntry>,
    /// Remaining records, if any; this does not resolve summarized values.
    pub next_cursor: Option<InspectionCursor>,
}

impl AuthoredSnapshot {
    /// Inspect retained declarations on the control thread without resolving files,
    /// expanding developments, or reading/changing live performance values.
    pub fn inspect(&self, query: &InspectionQuery) -> Result<InspectionPage, RpcError> {
        if !(1..=MAX_INSPECTION_ENTRIES).contains(&query.limit) {
            return Err(invalid("limit must be between 1 and 100"));
        }
        let offset = if let Some(cursor) = &query.cursor {
            if cursor.revision != self.revision {
                return Err(RpcError::revision_conflict(RevisionConflict {
                    expected: cursor.revision.clone(),
                    current: self.revision.clone(),
                    reason: if cursor.revision.session_id == self.revision.session_id {
                        ConflictReason::StaleRevision
                    } else {
                        ConflictReason::SessionReplaced
                    },
                }));
            }
            if cursor.selection != query.selection {
                return Err(invalid("continuation belongs to a different selection"));
            }
            cursor.offset
        } else {
            0
        };
        if !fits(&query.selection, 2048) {
            return Err(invalid(
                "selection exceeds 2048 JSON bytes; use a shorter authored pointer",
            ));
        }
        let document = serde_json::to_value(&self.document)
            .map_err(|e| RpcError::new(RpcErrorCode::Internal, e.to_string()))?;
        let entries = selection::select(&document, &query.selection)?;
        if offset > entries.len() {
            return Err(invalid("continuation offset exceeds selection"));
        }
        let mut page = InspectionPage {
            revision: self.revision.clone(),
            source_path: self.source_path.clone(),
            selection: query.selection.clone(),
            total_entries: entries.len(),
            offset,
            entries: Vec::new(),
            next_cursor: None,
        };
        for entry in entries.into_iter().skip(offset).take(query.limit) {
            page.entries.push(entry);
            page.update_cursor();
            if !fits(&page, MAX_INSPECTION_BYTES) {
                page.entries.pop();
                page.update_cursor();
                break;
            }
        }
        if (page.entries.is_empty() && offset < page.total_entries)
            || !fits(&page, MAX_INSPECTION_BYTES)
        {
            return Err(RpcError::new(RpcErrorCode::ResponseTooLarge,
                "inspection metadata exceeds 16384 bytes; narrow the selection or use get_invention delivery=file"));
        }
        Ok(page)
    }
}

impl InspectionPage {
    fn update_cursor(&mut self) {
        let offset = self.offset + self.entries.len();
        self.next_cursor = (offset < self.total_entries).then(|| InspectionCursor {
            revision: self.revision.clone(),
            selection: self.selection.clone(),
            offset,
        });
    }
}

pub(super) fn invalid(message: &str) -> RpcError {
    RpcError::new(RpcErrorCode::InvalidRequest, message)
}

// Count bytes without allocating a serialized copy, stopping at the budget.
fn fits(value: &impl Serialize, limit: usize) -> bool {
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_sub(bytes.len())
                .ok_or_else(|| std::io::Error::other("budget exceeded"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget(limit), value).is_ok()
}

fn entry(pointer: String, value: &Value, coverage: InspectionCoverage) -> InspectionEntry {
    if fits(value, MAX_INSPECTION_VALUE_BYTES) {
        InspectionEntry {
            pointer,
            coverage,
            value: Some(value.clone()),
            string_range: None,
        }
    } else {
        InspectionEntry {
            pointer,
            coverage: InspectionCoverage::Omitted,
            value: None,
            string_range: None,
        }
    }
}

// Option<Value>'s usual deserializer conflates an authored null with a missing
// value. Coverage and round trips must preserve that distinction.
fn present_value<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}
