//! A development's inside as the running piece built it: the answer to
//! [`RpcCommand::GetDevelopment`](super::RpcCommand::GetDevelopment).

use super::authored_snapshot::LimitedWriter;
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

impl DevelopmentSnapshot {
    /// The snapshot, refused with `ResponseTooLarge` when its JSON would
    /// exceed [`MAX_DEVELOPMENT_BYTES`].
    pub fn bounded(self) -> Result<Self, RpcError> {
        let mut writer = LimitedWriter {
            bytes: Vec::new(),
            limit: MAX_DEVELOPMENT_BYTES,
        };
        match serde_json::to_writer(&mut writer, &self) {
            Ok(()) => Ok(self),
            Err(error) if error.is_io() => Err(RpcError::new(
                RpcErrorCode::ResponseTooLarge,
                format!(
                    "development '{}' exceeds {MAX_DEVELOPMENT_BYTES} bytes; read it with \
                     inspect_invention or save_invention instead",
                    self.path.join("/")
                ),
            )),
            Err(error) => Err(RpcError::new(RpcErrorCode::Internal, error.to_string())),
        }
    }
}
