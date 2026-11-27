//! The differences as one host reads them: measured from the piece as last
//! loaded or saved, one sentence on the whole, and pages that hold the
//! inspection limits with the revision in the cursor.

use super::{differences, invalid, DiffEntry, PerformedValue, SoundingValue};
use crate::rpc::inspection::fits;
use crate::rpc::{RevisionConflict, RpcError, RpcErrorCode, RuntimeRevision};
use crate::rpc::{MAX_INSPECTION_BYTES, MAX_INSPECTION_ENTRIES};
use crate::Invention;
use serde::{Deserialize, Serialize};

/// What a difference is measured from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DiffBaseline {
    /// The piece as it was loaded; nothing has been saved since.
    Loaded,
    /// The piece as it was last saved.
    Saved,
}

/// One read of the differences. Every field is optional.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DiffQuery {
    /// Defaults to 50; must be between 1 and 100.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// The previous page's `next_cursor`; omit on the first read.
    #[serde(default)]
    pub cursor: Option<DiffCursor>,
}

impl Default for DiffQuery {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            cursor: None,
        }
    }
}

fn default_limit() -> usize {
    50
}

/// Continuation bound to the revision that produced it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct DiffCursor {
    /// Session identity and authored revision of this read.
    pub revision: RuntimeRevision,
    /// Zero-based record offset across changes, then performed values.
    pub offset: usize,
}

/// One page of the differences since the baseline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct DiffPage {
    /// Session identity and authored revision of this read.
    pub revision: RuntimeRevision,
    /// What the changes are measured from.
    pub baseline: DiffBaseline,
    /// One or two sentences on the whole difference, every page alike.
    pub summary: String,
    /// Document changes across all pages.
    pub total_changes: usize,
    /// Performed values across all pages.
    pub total_performed: usize,
    /// Zero-based record offset across changes, then performed values.
    pub offset: usize,
    pub changes: Vec<DiffEntry>,
    /// Values a save would not keep.
    pub performed: Vec<PerformedValue>,
    /// Remaining records, if any.
    pub next_cursor: Option<DiffCursor>,
}

/// What the host holds for one read: the baseline document and its kind,
/// the current document at `revision`, and the values performed (each
/// control's value as it sounds, for the controls written as performances).
pub struct DiffSource<'a> {
    pub baseline: (&'a Invention, DiffBaseline),
    pub current: &'a Invention,
    pub sounding: &'a [SoundingValue],
    pub revision: &'a RuntimeRevision,
}

impl DiffSource<'_> {
    /// One page of the differences, as `query` asks.
    pub fn page(&self, query: &DiffQuery) -> Result<DiffPage, RpcError> {
        if !(1..=MAX_INSPECTION_ENTRIES).contains(&query.limit) {
            return Err(invalid("limit must be between 1 and 100"));
        }
        let offset = match &query.cursor {
            Some(cursor) => {
                if let Some(conflict) = RevisionConflict::between(&cursor.revision, self.revision) {
                    return Err(RpcError::revision_conflict(conflict));
                }
                cursor.offset
            }
            None => 0,
        };
        let found = differences(self.baseline.0, self.current, self.sounding)?;
        let (changes, performed) = (found.changes, found.performed);
        let total = changes.len() + performed.len();
        if offset > total {
            return Err(invalid("continuation offset exceeds the differences"));
        }
        let mut page = DiffPage {
            revision: self.revision.clone(),
            baseline: self.baseline.1,
            summary: summary(self.baseline.1, changes.len(), performed.len()),
            total_changes: changes.len(),
            total_performed: performed.len(),
            offset,
            changes: Vec::new(),
            performed: Vec::new(),
            next_cursor: None,
        };
        let records = (changes.into_iter().map(Ok))
            .chain(performed.into_iter().map(Err))
            .skip(offset);
        for record in records.take(query.limit) {
            let is_change = record.is_ok();
            match record {
                Ok(change) => page.changes.push(change),
                Err(value) => page.performed.push(value),
            }
            page.update_cursor(total);
            if !fits(&page, MAX_INSPECTION_BYTES) {
                if is_change {
                    page.changes.pop();
                } else {
                    page.performed.pop();
                }
                page.update_cursor(total);
                break;
            }
        }
        let read = page.changes.len() + page.performed.len();
        if (read == 0 && offset < total) || !fits(&page, MAX_INSPECTION_BYTES) {
            return Err(RpcError::new(
                RpcErrorCode::ResponseTooLarge,
                "a difference exceeds 16384 bytes; read the piece with get_invention",
            ));
        }
        Ok(page)
    }
}

impl DiffPage {
    fn update_cursor(&mut self, total: usize) {
        let offset = self.offset + self.changes.len() + self.performed.len();
        self.next_cursor = (offset < total).then(|| DiffCursor {
            revision: self.revision.clone(),
            offset,
        });
    }
}

/// The page's sentence: what changed since the baseline, and whether
/// anything sounds that a save would not keep.
fn summary(baseline: DiffBaseline, changes: usize, performed: usize) -> String {
    let since = match baseline {
        DiffBaseline::Loaded => "since the piece was loaded",
        DiffBaseline::Saved => "since the piece was last saved",
    };
    let plural = |n: usize, one: &str, many: &str| match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    let document = match changes {
        0 if performed == 0 => return format!("Nothing has changed {since}."),
        0 => format!("The document has not changed {since}."),
        n => format!("{} {since}.", plural(n, "change", "changes")),
    };
    match performed {
        0 => document,
        n => format!(
            "{document} {} only performed and not in the document; a save would not keep {}.",
            plural(n, "value sounds", "values sound"),
            if n == 1 { "it" } else { "them" }
        ),
    }
}
