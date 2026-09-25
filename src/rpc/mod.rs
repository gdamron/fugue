//! Typed runtime RPC schema shared by Fugue clients.
//!
//! This module defines the JSON payloads used by future daemon transports. It
//! intentionally contains no socket, WebSocket, or MCP server implementation.

mod authored_snapshot;
mod inspection;
pub use inspection::{
    InspectionCoverage, InspectionCursor, InspectionEntry, InspectionPage, InspectionQuery,
    InspectionSelection, MAX_INSPECTION_BYTES, MAX_INSPECTION_ENTRIES, MAX_INSPECTION_VALUE_BYTES,
};
mod discovery;
mod error;
mod event;
mod identity;
mod package;
mod request;
mod response;
mod revision;
mod snapshot;
mod spectrogram;
pub use authored_snapshot::{
    AuthoredSnapshot, SnapshotDelivery, MAX_FILE_SNAPSHOT_BYTES, MAX_INLINE_SNAPSHOT_BYTES,
};
pub use discovery::{
    check_discovery_size, DescribeModuleQuery, MetadataSource, ModuleDescription, ModuleTypeDetail,
    ModuleTypeIndex, ModuleTypeInfo, ModuleTypeList, ModuleTypeQuery, RegistryScope, TypeDetail,
    MAX_DISCOVERY_RESPONSE_BYTES, MAX_DISCOVERY_TYPES, MODULE_DISCOVERY_SCHEMA_VERSION,
};
pub use error::{RpcError, RpcErrorCode};
pub use event::{
    EventPage, MeterReading, RpcEvent, RpcEventPayload, RpcEventSink, RpcSubscriptionTopic,
    SeqEvent, SinkStatusState,
};
pub use identity::{verify_daemon_identity, BuildFingerprint, DaemonIdentity, IdentityMismatch};
pub use package::{PackageInfo, PackageList, PackageSource};
pub use request::{ControlWrite, PackageInstallRequest, RpcCommand, RpcRequest, RpcRequestPayload};
pub use response::{ReloadMode, ReloadOutcome, RpcResponse, RpcResponsePayload, SaveReport};
pub use revision::{
    ConflictReason, ControlWriteIntent, RevisionConflict, RevisionTracker, RuntimeRevision,
};
pub use snapshot::{
    RuntimeControlSnapshot, RuntimeFullSnapshot, RuntimeModuleSnapshot, RuntimePortInfo,
};
pub use spectrogram::{
    SpectrogramBinSpacing, SpectrogramDbReference, SpectrogramDbScale, SpectrogramEncoding,
    SpectrogramFrequencyAxis, SpectrogramLimits, SpectrogramMagnitudes, SpectrogramProvenance,
    SpectrogramStreamMeta, SpectrogramTile, SpectrogramWindow,
};

/// Current runtime RPC schema version.
pub const RPC_SCHEMA_VERSION: u32 = 1;

pub fn validate_schema_version(schema_version: u32) -> Result<(), RpcError> {
    if schema_version == RPC_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(RpcError::new(
            RpcErrorCode::IncompatibleSchemaVersion,
            format!(
                "incompatible RPC schema version: client sent {}, server requires {}",
                schema_version, RPC_SCHEMA_VERSION
            ),
        ))
    }
}

#[cfg(feature = "rpc-schema")]
pub mod schema {
    use super::{RpcEvent, RpcRequest, RpcResponse};
    use schemars::{schema_for, Schema};

    pub fn runtime_rpc_schema() -> Schema {
        schema_for!((RpcRequest, RpcResponse, RpcEvent))
    }
}

#[cfg(test)]
mod tests;
