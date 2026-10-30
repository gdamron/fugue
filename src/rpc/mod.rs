//! Typed runtime RPC schema shared by Fugue clients.
//!
//! This module defines the JSON payloads used by future daemon transports. It
//! intentionally contains no socket, WebSocket, or MCP server implementation.

mod authored_snapshot;
mod control_timing;
mod inspection;
pub use inspection::{
    InspectionCoverage, InspectionCursor, InspectionEntry, InspectionPage, InspectionQuery,
    InspectionSelection, MAX_INSPECTION_BYTES, MAX_INSPECTION_ENTRIES, MAX_INSPECTION_VALUE_BYTES,
};
mod discovery;
mod durability;
mod edits;
mod error;
mod event;
mod focus;
mod identity;
mod notes;
mod package;
mod pending;
mod receipt;
mod recovery;
mod request;
mod response;
mod revision;
mod snapshot;
mod spectrogram;
mod timed;
pub use authored_snapshot::{
    AuthoredSnapshot, SnapshotDelivery, MAX_FILE_SNAPSHOT_BYTES, MAX_INLINE_SNAPSHOT_BYTES,
};
pub use control_timing::{ControlRequestSettled, ControlTiming, ControlWhen};
pub use discovery::{
    check_discovery_size, DescribeModuleQuery, MetadataSource, ModuleDescription, ModuleTypeDetail,
    ModuleTypeIndex, ModuleTypeInfo, ModuleTypeList, ModuleTypeQuery, RegistryScope, TypeDetail,
    MAX_DISCOVERY_RESPONSE_BYTES, MAX_DISCOVERY_TYPES, MODULE_DISCOVERY_SCHEMA_VERSION,
};
pub use durability::{SessionDurability, SessionSaveState};
pub(crate) use edits::non_finite_refusal;
pub use edits::{
    check_edit_batch, ApplyEditsReport, ControlWriteFailure, EditFailure, EditFailureReason,
    EditOp, StructuralEdit, WrittenControl, MAX_EDITS_PER_BATCH, MAX_EDIT_NAME_BYTES,
};
pub use error::{RpcError, RpcErrorCode};
pub use notes::{NotePage, NoteTimeUnit, SeqNote};
pub use event::{
    EventPage, MeterReading, RpcEvent, RpcEventPayload, RpcEventSink, RpcSubscriptionTopic,
    SeqEvent, SinkStatusState,
};
pub use focus::{
    Focus, FocusBars, FocusConnection, FocusIntent, FocusNote, FocusTarget, FocusTimeUnit,
    MAX_FOCUS_ITEMS, MAX_FOCUS_NAME_BYTES,
};
pub use identity::{verify_daemon_identity, BuildFingerprint, DaemonIdentity, IdentityMismatch};
pub use package::{
    PackageInfo, PackageInstallReport, PackageInstallRequest, PackageList, PackageListQuery,
    PackageSource, MAX_PACKAGE_DIAGNOSTICS, MAX_PACKAGE_LIST_BYTES, MAX_PACKAGE_SUMMARY_BYTES,
    PACKAGE_SCHEMA_VERSION,
};
pub use pending::{
    PendingCounts, PendingDetail, PendingRequest, PendingRequests, PendingSource, PendingWait,
    COMPACT_PENDING_ITEMS,
};
pub use receipt::{AppliedControl, ControlRequestState, MutationReceipt, MutationReply};
pub(crate) use recovery::{truncate_on_char_boundary, MODULE_ERROR_BYTES};
pub use recovery::{
    Admission, MutationLedger, MutationTicket, PendingMutation, ReplayPolicy,
    MAX_MUTATION_ID_BYTES, MAX_RECORDED_MESSAGE_BYTES, MUTATION_LEDGER_CAPACITY,
};
pub use request::{ControlWrite, RequestSource, RpcCommand, RpcRequest, RpcRequestPayload};
pub use response::{ReloadMode, ReloadOutcome, RpcResponse, RpcResponsePayload, SaveReport};
pub use revision::{
    ConflictReason, ControlWriteIntent, RevisionConflict, RevisionTracker, RuntimeRevision,
};
pub use snapshot::{
    RuntimeControlSnapshot, RuntimeFullSnapshot, RuntimeModuleSnapshot, RuntimePortInfo,
    TimelineContext,
};
pub use spectrogram::{
    SpectrogramBinSpacing, SpectrogramDbReference, SpectrogramDbScale, SpectrogramEncoding,
    SpectrogramFrequencyAxis, SpectrogramLimits, SpectrogramMagnitudes, SpectrogramProvenance,
    SpectrogramStreamMeta, SpectrogramTile, SpectrogramWindow,
};

pub use timed::{EditTiming, EditWhen, RequestLanding, RequestState, RequestStatus};

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
