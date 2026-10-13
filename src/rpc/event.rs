//! Server-pushed events, subscription topics, and the polled event log.

use super::{
    RpcError, RuntimeFullSnapshot, SpectrogramStreamMeta, SpectrogramTile, RPC_SCHEMA_VERSION,
};
use crate::ControlValue;
use serde::{Deserialize, Serialize};

/// A server-pushed event envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct RpcEvent {
    pub schema_version: u32,
    #[serde(flatten)]
    pub payload: RpcEventPayload,
}

impl RpcEvent {
    pub fn new(payload: RpcEventPayload) -> Self {
        Self {
            schema_version: RPC_SCHEMA_VERSION,
            payload,
        }
    }
}

/// Event stream topics clients can subscribe to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RpcSubscriptionTopic {
    ControlChanges,
    MeterLevels,
    /// Display-rate spectrogram frames. Like meter levels, these are streamed
    /// only: they are far too frequent for the polled event log.
    Spectrograms,
    AgentActivity,
    SinkStatus,
    Errors,
    Topology,
}

/// Runtime events emitted by daemon transports.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RpcEventPayload {
    ControlChanged {
        module_id: String,
        key: String,
        value: ControlValue,
    },
    MeterLevel {
        sink_id: String,
        left_peak: f32,
        right_peak: f32,
    },
    AgentActivity {
        module_id: String,
        activity: String,
    },
    SinkStatus {
        sink_id: String,
        status: SinkStatusState,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Announces a spectrogram stream: sent when a client subscribes and
    /// whenever analysis restarts under a new `stream_id`.
    SpectrogramStream(SpectrogramStreamMeta),
    /// One run of spectrogram frames.
    SpectrogramTile(SpectrogramTile),
    Error(RpcError),
    /// The graph or playback state changed. Carries nothing: polled history
    /// never holds a graph, so its size does not grow with the invention.
    /// Read a snapshot for the new state.
    TopologyChanged,
    /// The whole runtime state. Never retained in polled history; streamed
    /// only to a subscriber, right after a `TopologyChanged` it delivered.
    Snapshot(RuntimeFullSnapshot),
}

/// Minimal sink interface for transports that collect or broadcast RPC events.
pub trait RpcEventSink: Send + Sync {
    fn emit(&self, event: RpcEvent);
}

/// One output source's latest sampled peak levels, in a [`GetMeters`] reply.
///
/// The same values are also broadcast continuously as
/// [`RpcEventPayload::MeterLevel`]; this is the pull view for clients that do
/// not hold a streaming socket (FUG-239 #5).
///
/// [`GetMeters`]: super::RpcRequestPayload::GetMeters
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct MeterReading {
    pub sink_id: String,
    pub left_peak: f32,
    pub right_peak: f32,
}

/// One event in an [`EventPage`], tagged with its monotonic sequence number so
/// a polling client can request everything after the last it saw.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SeqEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub payload: RpcEventPayload,
}

/// Reply to [`RpcRequestPayload::PollEvents`]: the retained events newer than
/// the requested cursor, plus the cursor to poll from next.
///
/// The log is bounded, so a slow poller can miss events. It is then told so
/// explicitly: `resync_required` is set and `events` is empty. To recover, read
/// state again (a snapshot), then poll from this page's `latest_seq`. Every
/// event is emitted after the change it describes has applied, so the snapshot
/// already reflects everything up to `latest_seq`, and replaying later events
/// on top of it is idempotent.
///
/// [`RpcRequestPayload::PollEvents`]: super::RpcRequestPayload::PollEvents
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct EventPage {
    /// Events newer than the cursor, oldest first. Empty when
    /// `resync_required` is set.
    pub events: Vec<SeqEvent>,
    /// Newest sequence number the daemon has assigned. Pass it as the next
    /// poll's `after`. Sequence numbers are not small counters: a daemon may
    /// start anywhere, so that a cursor from an earlier daemon is never
    /// mistaken for a current one.
    pub latest_seq: u64,
    /// The cursor cannot be continued: events after it were evicted or never
    /// retained, or it came from a different daemon. Whatever the caller holds
    /// may be stale; read state again before trusting it.
    pub resync_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SinkStatusState {
    Starting,
    Running,
    Stopped,
    Error,
}
