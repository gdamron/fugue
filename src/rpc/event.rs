//! Server-pushed events, subscription topics, and the polled event log.

use super::{
    ControlRequestSettled, RequestStatus, RpcError, RuntimeFullSnapshot, SpectrogramStreamMeta,
    SpectrogramTile, RPC_SCHEMA_VERSION,
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
    /// Frames of the scopes opened for streaming (see
    /// [`ScopeFrame`](super::ScopeFrame)). Display-rate like spectrograms,
    /// so streamed only to clients that name this topic.
    Scopes,
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
    /// A control write applied, carrying the value as applied.
    ControlChanged {
        module_id: String,
        key: String,
        value: ControlValue,
        /// The request it was submitted as (see
        /// [`AppliedControl::request`](super::AppliedControl::request)),
        /// announced once the request applies. `None` for a write that
        /// applied as it was made.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<String>,
    },
    /// The output's levels over the latest meter window. Streamed only, at
    /// the daemon's meter rate; never retained in polled history.
    MeterLevel(MeterReading),
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
    /// A streamed scope's bins since its last frame. Streamed only, at the
    /// daemon's frame rate; never retained in polled history.
    ScopeFrame(super::ScopeFrame),
    Error(RpcError),
    /// The graph or playback state changed. Carries nothing: polled history
    /// never holds a graph, so its size does not grow with the invention.
    /// Read a snapshot for the new state.
    TopologyChanged,
    /// The whole runtime state. Never retained in polled history; streamed
    /// only to a subscriber, right after a `TopologyChanged` it delivered.
    Snapshot(RuntimeFullSnapshot),
    /// A timed request changed state: queued, landed, adopted, refused,
    /// expired or cancelled.
    RequestChanged(RequestStatus),
    /// A timed control write settled: applied, superseded, overridden,
    /// refused or expired. One that applied is also announced by
    /// `ControlChanged`.
    ControlRequestSettled(ControlRequestSettled),
    /// The session's pinned controls changed: an edit, or a graph change
    /// that dropped some. Values are not carried; read them with `GetPins`.
    PinsChanged(super::PinList),
}

/// Minimal sink interface for transports that collect or broadcast RPC events.
pub trait RpcEventSink: Send + Sync {
    fn emit(&self, event: RpcEvent);
}

/// One output's levels over one meter window: a [`GetMeters`] reply entry,
/// and the body of each streamed [`RpcEventPayload::MeterLevel`].
///
/// The daemon meters one output, the mixed master (`sink_id` `"master"`):
/// that is the intended extent, and there are no per-module meters. It drains
/// the meter on a fixed cadence (about 10 Hz), streams each window once, and
/// keeps the latest for `GetMeters`, so polling faster than the cadence
/// returns the same window again, with the same `audio_clock_frame`.
///
/// Channels are `left` and `right` of the stereo mix. Levels are linear
/// amplitude, where 1.0 is full scale (0 dBFS), measured on the mixed block
/// before the output stage clamps it, so overs read above 1.0. Peak and RMS
/// cover the same window: every frame rendered since the previous reading.
///
/// Fields after the peaks were added later and are optional on the wire: a
/// daemon that predates them omits them, and a client that predates them
/// ignores them. A window with no audio in it reads peaks of 0.0 and no
/// RMS; RMS is also absent when the window's sum of squares was not finite.
///
/// [`GetMeters`]: super::RpcRequestPayload::GetMeters
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct MeterReading {
    /// The metered output: `"master"`, the mixed output of every sink.
    pub sink_id: String,
    /// Largest absolute sample in the window, left channel.
    pub left_peak: f32,
    /// Largest absolute sample in the window, right channel.
    pub right_peak: f32,
    /// Root mean square over the window, left channel. Never above the peak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_rms: Option<f32>,
    /// Root mean square over the window, right channel. Never above the peak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_rms: Option<f32>,
    /// Where the window ended on the audio clock: frames the invention had
    /// rendered, counted from zero when it started playing. It never goes
    /// back while one invention plays (reloads included), and restarts from
    /// zero whenever playback starts again. Windows tile it: each starts
    /// where the previous one ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_clock_frame: Option<u64>,
    /// `audio_clock_frame` in milliseconds at `sample_rate`, rounded to the
    /// microsecond.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_clock_ms: Option<f64>,
    /// Frames per second on the audio clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

impl MeterReading {
    /// The reading for one window of `sink_id`'s levels.
    pub fn new(sink_id: impl Into<String>, levels: &crate::MasterLevels) -> Self {
        Self {
            sink_id: sink_id.into(),
            left_peak: levels.left_peak,
            right_peak: levels.right_peak,
            left_rms: levels.left_rms,
            right_rms: levels.right_rms,
            audio_clock_frame: Some(levels.end_frame),
            audio_clock_ms: Some(levels.end_ms()),
            sample_rate: Some(levels.sample_rate),
        }
    }
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
