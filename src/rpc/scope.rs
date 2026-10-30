//! A live scope of one module's output: the wire side of an output tap.
//!
//! A client opens a scope on a module ([`RpcRequestPayload::OpenScope`]) and
//! gets a [`ScopeInfo`] naming it. The daemon reads the module's output tap
//! (see [`OutputTap`](crate::OutputTap)) every
//! [`ScopeInfo::frame_interval_ms`] and hands what it read on as
//! [`ScopeFrame`]s: pushed to subscribers of the `scopes` topic for a
//! streamed scope, or kept for the next [`RpcRequestPayload::ReadScope`] for
//! a polled one. [`RpcRequestPayload::CloseScope`] closes it, and the daemon
//! closes it too when the client that opened it goes away, when a polled
//! scope goes unread for [`ScopeInfo::idle_close_ms`], or when its tap
//! closes (the module was removed or rebuilt, or the invention stopped).
//!
//! A frame is the waveform view's min/max bin format: per channel, the
//! lowest and highest sample over each run of `samples_per_bin` samples.
//! Only these bins leave the daemon, never the audio itself.
//!
//! [`RpcRequestPayload::OpenScope`]: super::RpcRequestPayload::OpenScope
//! [`RpcRequestPayload::ReadScope`]: super::RpcRequestPayload::ReadScope
//! [`RpcRequestPayload::CloseScope`]: super::RpcRequestPayload::CloseScope

use serde::{Deserialize, Serialize};

use crate::OutputBins;

/// Asks the daemon to open a scope on a module's output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct OpenScope {
    pub module_id: String,
    /// The outputs to read, one or two. None names the module's audio
    /// (`audio_left` and `audio_right`, else `audio`, else its first output).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    /// Samples each bin covers. The daemon rounds it up to a whole number
    /// of the tap's own bins and says what it chose in
    /// [`ScopeInfo::samples_per_bin`]; it picks a width when none is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples_per_bin: Option<u32>,
    /// How frames reach the client; polled when omitted.
    #[serde(default)]
    pub delivery: ScopeDelivery,
}

/// How a scope's frames reach the client.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScopeDelivery {
    /// Pushed as `scope_frame` events to subscribers of the `scopes` topic,
    /// at the daemon's frame rate. For a client holding a live socket.
    Stream,
    /// Kept by the daemon until the client reads them with `read_scope`,
    /// as often as it can: for a client that can only make requests.
    #[default]
    Poll,
}

/// How a [`ScopeFrame`]'s bins are written. Named from the first version,
/// so a compact form can be added later and a client that doesn't know it
/// says so rather than misreading it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ScopeEncoding {
    /// `min` and `max` are JSON arrays of numbers, `null` for a gap.
    #[default]
    F32Json,
    /// An encoding this build does not know. Never produced.
    #[serde(other)]
    Unsupported,
}

/// An open scope, in reply to `open_scope`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ScopeInfo {
    /// Names the scope in `read_scope`, `close_scope` and its frames.
    pub scope_id: String,
    pub module_id: String,
    /// The outputs read, one per channel, in channel order.
    pub ports: Vec<String>,
    /// Samples a second, to put frames' times and bin widths in seconds.
    pub sample_rate: u32,
    /// Samples each bin covers.
    pub samples_per_bin: u32,
    pub delivery: ScopeDelivery,
    pub encoding: ScopeEncoding,
    /// How often the daemon reads the tap: a streamed scope's frame rate.
    pub frame_interval_ms: u32,
    /// A polled scope closes once it has gone this long without a read.
    /// None for a streamed scope, which lasts as long as the connection
    /// that opened it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_close_ms: Option<u32>,
}

/// One frame of a scope: what each output did since the frame before.
///
/// The fields match the waveform view's excerpt (`module_id`,
/// `sample_rate`, `samples_per_bin`, `origin_frame`, `channels`), so an
/// adapter hands a frame over with nothing to convert. Values are the
/// module's output as it produced it, unscaled and unclipped: for audio,
/// 1.0 is full scale. A port whose range is not known is read as audio.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ScopeFrame {
    pub scope_id: String,
    pub module_id: String,
    pub sample_rate: u32,
    pub samples_per_bin: u32,
    /// The first bin's first sample, counted from zero as the invention
    /// started playing.
    pub origin_frame: u64,
    /// One per output, in [`ScopeInfo::ports`] order, all the same length.
    pub channels: Vec<ScopeChannel>,
    /// Samples between the end of the frame before and `origin_frame` that
    /// no bin covers: the client fell behind, or the module wasn't running.
    pub lost: u64,
    /// False once the scope has closed: its last frame. Open it again to
    /// follow the module now under that id.
    pub open: bool,
}

/// One output's bins, oldest first: `min[n]` and `max[n]` bound bin `n`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct ScopeChannel {
    /// The output port it reads.
    pub label: String,
    /// `null` with `max` for a bin that holds no value (a gap); never one
    /// without the other.
    pub min: Vec<Option<f32>>,
    pub max: Vec<Option<f32>>,
}

impl ScopeFrame {
    /// The frame for one read of a scope's tap. A bin with no finite value
    /// (every sample NaN, or infinite) is a gap: `null` in `min` and `max`
    /// together, never NaN, which JSON can't carry.
    pub fn from_bins(scope_id: impl Into<String>, bins: OutputBins) -> Self {
        let channels = (bins.ports.into_iter())
            .zip(bins.channels)
            .map(|(label, channel)| {
                let (min, max) = (channel.min.iter().zip(&channel.max))
                    .map(|(&min, &max)| match min.is_finite() && max.is_finite() {
                        true => (Some(min), Some(max)),
                        false => (None, None),
                    })
                    .unzip();
                ScopeChannel { label, min, max }
            })
            .collect();
        Self {
            scope_id: scope_id.into(),
            module_id: bins.module_id,
            sample_rate: bins.sample_rate,
            samples_per_bin: bins.samples_per_bin,
            origin_frame: bins.origin_frame,
            channels,
            lost: bins.lost,
            open: bins.open,
        }
    }

    /// Bins per channel.
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, |channel| channel.min.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
