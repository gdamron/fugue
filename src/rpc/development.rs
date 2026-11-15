//! A development's inside as the running piece built it: the answer to
//! [`RpcCommand::GetDevelopment`](super::RpcCommand::GetDevelopment).

use super::{RpcError, RpcErrorCode, RuntimeRevision, MAX_INLINE_SNAPSHOT_BYTES};
use crate::invention::layout::Position;
use crate::Invention;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most UTF-8 JSON bytes one development reply carries, the inline
/// snapshot's bound (64 KiB).
pub const MAX_DEVELOPMENT_BYTES: usize = MAX_INLINE_SNAPSHOT_BYTES;

/// How the document that declares a development names its definition.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentDeclaration {
    /// An inline `definition`.
    Inline,
    /// A `path` to a development file.
    Path,
    /// A catalog `ref`, such as an installed package.
    Ref,
}

/// Where a move inside a development is saved (see `move_modules`'s
/// `development`), so whether it can be arranged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DevelopmentSavedIn {
    /// Inline in the loaded piece's own document.
    Piece,
    /// In the development file at `path` on the daemon host, which every
    /// piece that uses the file shares.
    File { path: String },
    /// In the loaded piece's own layout, as overrides layered on the
    /// original, which is never written: the piece borrows it from an
    /// installed package (or a file a frozen lockfile locks). Removing the
    /// overrides goes back to the author's arrangement.
    Override,
    /// Never written here, for `reason`: the piece itself is an installed
    /// package's (an installed example), so nothing inside it is saved.
    ReadOnly { reason: String },
}

/// One development's definition as the running piece loaded it, whether
/// declared inline, by path or by catalog reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct DevelopmentSnapshot {
    /// The daemon session and authoring revision the piece was at.
    pub revision: RuntimeRevision,
    /// The development read, as the request named it.
    pub path: Vec<String>,
    pub declared: DevelopmentDeclaration,
    /// Where a move inside it is saved.
    pub saved_in: DevelopmentSavedIn,
    /// Its modules, connections, exposed inputs, outputs and controls, and
    /// layout, with the developments it declares itself. The layout is the
    /// author's with the piece's overrides on top.
    pub definition: Invention,
    /// The piece's overrides inside it, by module id: where the arrangement
    /// shown differs from the original. Empty when it does not.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, Position>,
    /// The module ids whose overrides the piece's latest load or reload
    /// dropped: the development no longer has them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides_dropped: Vec<String>,
}

/// The piece's overrides inside one development, for its snapshot.
#[derive(Debug, Clone, Default)]
pub struct DevelopmentOverrides {
    pub positions: BTreeMap<String, Position>,
    pub dropped: Vec<String>,
}

/// [`DevelopmentSnapshot`] borrowed, serialized the same way, so its size
/// is measured before anything is copied.
#[derive(Serialize)]
struct Borrowed<'a> {
    revision: &'a RuntimeRevision,
    path: &'a [String],
    declared: DevelopmentDeclaration,
    saved_in: &'a DevelopmentSavedIn,
    definition: &'a Invention,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    overrides: &'a BTreeMap<String, Position>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    overrides_dropped: &'a [String],
}

impl DevelopmentSnapshot {
    /// The snapshot of `definition`, refused with `ResponseTooLarge` when
    /// its JSON would exceed [`MAX_DEVELOPMENT_BYTES`].
    pub fn bounded(
        revision: RuntimeRevision,
        path: &[String],
        declared: DevelopmentDeclaration,
        saved_in: DevelopmentSavedIn,
        definition: &Invention,
        overrides: DevelopmentOverrides,
    ) -> Result<Self, RpcError> {
        let borrowed = Borrowed {
            revision: &revision,
            path,
            declared,
            saved_in: &saved_in,
            definition,
            overrides: &overrides.positions,
            overrides_dropped: &overrides.dropped,
        };
        let mut counted = Counted(0);
        match serde_json::to_writer(&mut counted, &borrowed) {
            Ok(()) => Ok(Self {
                revision,
                path: path.to_vec(),
                declared,
                saved_in,
                definition: definition.clone(),
                overrides: overrides.positions,
                overrides_dropped: overrides.dropped,
            }),
            Err(error) if error.is_io() => Err(RpcError::new(
                RpcErrorCode::ResponseTooLarge,
                format!(
                    "development '{}' exceeds {MAX_DEVELOPMENT_BYTES} bytes; {}",
                    path.join("/"),
                    match declared {
                        DevelopmentDeclaration::Inline =>
                            "read its definition in parts with inspect_invention",
                        DevelopmentDeclaration::Path =>
                            "read the file its declaration names on the daemon host",
                        DevelopmentDeclaration::Ref => "read it from its package's source",
                    }
                ),
            )),
            Err(error) => Err(RpcError::new(RpcErrorCode::Internal, error.to_string())),
        }
    }
}

/// Counts the bytes written, failing past the bound.
struct Counted(usize);

impl std::io::Write for Counted {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_DEVELOPMENT_BYTES.saturating_sub(self.0) {
            return Err(std::io::Error::other("development size limit exceeded"));
        }
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
