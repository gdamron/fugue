//! A development's inside as the running piece built it: the answer to
//! [`RpcCommand::GetDevelopment`](super::RpcCommand::GetDevelopment).

use super::{RpcError, RpcErrorCode, RuntimeRevision, MAX_INLINE_SNAPSHOT_BYTES};
use crate::Invention;
use serde::{Deserialize, Serialize};

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
    /// Its modules, connections, exposed inputs, outputs and controls, and
    /// layout, with the developments it declares itself.
    pub definition: Invention,
}

/// [`DevelopmentSnapshot`] borrowed, serialized the same way, so its size
/// is measured before anything is copied.
#[derive(Serialize)]
struct Borrowed<'a> {
    revision: &'a RuntimeRevision,
    path: &'a [String],
    declared: DevelopmentDeclaration,
    definition: &'a Invention,
}

impl DevelopmentSnapshot {
    /// The snapshot of `definition`, refused with `ResponseTooLarge` when
    /// its JSON would exceed [`MAX_DEVELOPMENT_BYTES`].
    pub fn bounded(
        revision: RuntimeRevision,
        path: &[String],
        declared: DevelopmentDeclaration,
        definition: &Invention,
    ) -> Result<Self, RpcError> {
        let borrowed = Borrowed {
            revision: &revision,
            path,
            declared,
            definition,
        };
        let mut counted = Counted(0);
        match serde_json::to_writer(&mut counted, &borrowed) {
            Ok(()) => Ok(Self {
                revision,
                path: path.to_vec(),
                declared,
                definition: definition.clone(),
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
