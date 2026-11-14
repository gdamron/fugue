//! Portable authored snapshots; storage location never changes source resolution.

use super::{RpcError, RpcErrorCode, RuntimeRevision};
use crate::Invention;
use serde::{Deserialize, Serialize};
use std::io::Write;

/// Maximum UTF-8 JSON snapshot size for model-visible retrieval (64 KiB).
pub const MAX_INLINE_SNAPSHOT_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 JSON snapshot size for file transfer (16 MiB).
pub const MAX_FILE_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// Explicit delivery budget. File transfer travels over RPC to the client;
/// it never means a file written by the daemon.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SnapshotDelivery {
    /// Bounded full-document response intended for model context.
    Inline,
    /// Larger transfer intended for client-side filesystem materialization.
    File,
}

/// A point-in-time authored document, including the context needed to reload it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct AuthoredSnapshot {
    /// Exact daemon session and authoring revision that produced this document.
    pub revision: RuntimeRevision,
    /// Absolute source document path on the daemon host, independent of storage.
    /// None means the original document had no file-based source context.
    pub source_path: Option<String>,
    /// Retained declarations, including authored controls but excluding gestures.
    pub document: Invention,
}

impl AuthoredSnapshot {
    /// Capture a retained document using the save format's serialization.
    pub fn new(document: Invention, revision: RuntimeRevision) -> Result<Self, RpcError> {
        let source_path = document
            .source_path
            .as_ref()
            .map(|path| std::path::absolute(path).map(|path| path.to_string_lossy().into_owned()))
            .transpose()
            .map_err(|error| RpcError::new(RpcErrorCode::Internal, error.to_string()))?;
        Ok(Self {
            revision,
            source_path,
            document,
        })
    }

    /// Serialize without truncation, bounding the serialized buffer allocation.
    /// Budgets include document, source context and revision, before RPC/MCP escaping.
    pub fn to_json(&self, delivery: SnapshotDelivery) -> Result<Vec<u8>, RpcError> {
        let limit = match delivery {
            SnapshotDelivery::Inline => MAX_INLINE_SNAPSHOT_BYTES,
            SnapshotDelivery::File => MAX_FILE_SNAPSHOT_BYTES,
        };
        let mut writer = LimitedWriter {
            bytes: Vec::new(),
            limit,
        };
        if let Err(error) = serde_json::to_writer_pretty(&mut writer, self) {
            return Err(if error.is_io() {
                RpcError::new(RpcErrorCode::ResponseTooLarge, match delivery {
                    SnapshotDelivery::Inline => format!("snapshot exceeds {limit} bytes; request delivery=file to a client-accessible filesystem"),
                    SnapshotDelivery::File => format!("snapshot exceeds {limit} bytes; file transfer is unsupported at this size; use save_invention on the daemon and an explicit file-transfer mechanism"),
                })
            } else {
                RpcError::new(RpcErrorCode::Internal, error.to_string())
            });
        }
        Ok(writer.bytes)
    }
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("snapshot size limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> AuthoredSnapshot {
        let mut document = Invention::from_json(r#"{"modules":[],"connections":[]}"#).unwrap();
        document.source_path = Some(std::path::PathBuf::from("original/piece.json"));
        AuthoredSnapshot::new(
            document,
            RuntimeRevision {
                session_id: "session".into(),
                revision: 42,
            },
        )
        .unwrap()
    }

    #[test]
    fn limits_are_exact_and_overflow_is_actionable() {
        let mut snapshot = snapshot();
        snapshot.document.description = Some(String::new());
        let overhead = snapshot.to_json(SnapshotDelivery::Inline).unwrap().len();
        snapshot.document.description = Some("a".repeat(MAX_INLINE_SNAPSHOT_BYTES - overhead));
        assert_eq!(
            snapshot.to_json(SnapshotDelivery::Inline).unwrap().len(),
            MAX_INLINE_SNAPSHOT_BYTES
        );
        snapshot.document.description.as_mut().unwrap().push('a');
        let error = snapshot.to_json(SnapshotDelivery::Inline).unwrap_err();
        assert_eq!(error.code, RpcErrorCode::ResponseTooLarge);
        assert!(error.message.contains("delivery=file"));
        assert!(snapshot.to_json(SnapshotDelivery::File).is_ok());
        snapshot.document.description = Some("a".repeat(MAX_FILE_SNAPSHOT_BYTES - overhead));
        assert_eq!(
            snapshot.to_json(SnapshotDelivery::File).unwrap().len(),
            MAX_FILE_SNAPSHOT_BYTES
        );
        snapshot.document.description.as_mut().unwrap().push('a');
        assert_eq!(
            snapshot.to_json(SnapshotDelivery::File).unwrap_err().code,
            RpcErrorCode::ResponseTooLarge
        );
    }

    #[test]
    fn source_and_revision_survive_wire_round_trip() {
        let snapshot = snapshot();
        let decoded: AuthoredSnapshot =
            serde_json::from_slice(&snapshot.to_json(SnapshotDelivery::Inline).unwrap()).unwrap();
        assert_eq!(snapshot.revision, decoded.revision);
        assert_eq!(snapshot.source_path, decoded.source_path);
        assert!(std::path::Path::new(decoded.source_path.as_ref().unwrap()).is_absolute());
        assert!(decoded.document.source_path.is_none());
        assert!(!super::super::RpcCommand::GetInvention {
            delivery: SnapshotDelivery::File
        }
        .advances_revision());
    }
}
