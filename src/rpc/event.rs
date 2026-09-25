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
    TopologyChanged,
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

/// Reply to [`RpcRequestPayload::PollEvents`]: the buffered events newer than
/// the requested cursor, plus the newest sequence number to poll from next.
///
/// [`RpcRequestPayload::PollEvents`]: super::RpcRequestPayload::PollEvents
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct EventPage {
    pub events: Vec<SeqEvent>,
    /// Newest sequence number the daemon has assigned (0 when nothing has been
    /// emitted yet). Pass this as the next poll's `after`.
    pub latest_seq: u64,
    /// True when the cursor fell behind the bounded log and some events between
    /// it and the oldest retained event were evicted unseen.
    pub dropped: bool,
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
