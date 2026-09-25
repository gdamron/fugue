//! Wire types for display-rate spectrogram streams.
//!
//! A spectrogram stream is announced once, then delivered as a run of small
//! tiles. Analysis happens off the audio thread; these types carry the result
//! to viewers and say exactly how it was produced, so a view can label its
//! axes honestly rather than guessing.

use serde::{Deserialize, Serialize};

/// Analysis window applied to each frame before the transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpectrogramWindow {
    Hann,
    Hamming,
    Blackman,
    Rectangular,
}

/// How the magnitudes in a stream's tiles are encoded.
///
/// Named from the first stream onwards so a compact encoding can be added
/// without a schema version bump: a client reads this and picks a decoder,
/// or reports the stream as unsupported instead of misreading it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpectrogramEncoding {
    /// `magnitudes_db` is a JSON array of decibel values.
    F32Json,
}

/// What 0 dB means on a stream's scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpectrogramDbReference {
    /// Full scale: a sine peaking at ±1.0 reads 0 dB.
    Dbfs,
}

/// How the magnitudes were computed, so the analysis can be reproduced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SpectrogramProvenance {
    /// Sample rate of the analysed signal, in Hz.
    pub sample_rate: u32,
    /// Transform size in samples; always a power of two.
    pub fft_size: u32,
    /// Samples between the starts of consecutive frames.
    pub hop_size: u32,
    pub window: SpectrogramWindow,
    /// Which signal was analysed, such as `sink:master`.
    pub source: String,
}

/// The frequency axis: `bin_count` bins of `bin_hz` starting at `min_hz`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SpectrogramFrequencyAxis {
    pub min_hz: f32,
    pub bin_hz: f32,
    pub bin_count: u32,
}

/// The decibel scale every magnitude is reported on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SpectrogramDbScale {
    pub reference: SpectrogramDbReference,
    /// Silence. Quieter magnitudes are clamped here, so values stay finite.
    pub floor_db: f32,
    /// The loudest level a view needs to distinguish.
    pub ceiling_db: f32,
}

/// Finite limits a stream promises to stay within, so a viewer can size its
/// buffers once and never grow them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SpectrogramLimits {
    /// Frames a viewer is expected to keep; older frames may be discarded.
    pub history_frames: u32,
    /// Largest tile this stream will send.
    pub max_frames_per_tile: u32,
}

/// Everything fixed for the life of one analysis run.
///
/// Announced when a client subscribes and again whenever analysis restarts
/// (a new `stream_id`), which tells viewers to clear what they hold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SpectrogramStreamMeta {
    /// Identifies one continuous analysis run.
    pub stream_id: String,
    pub provenance: SpectrogramProvenance,
    pub frequency: SpectrogramFrequencyAxis,
    pub db: SpectrogramDbScale,
    pub limits: SpectrogramLimits,
    pub encoding: SpectrogramEncoding,
}

/// A run of consecutive analysis frames.
///
/// Frame `n` covers the samples starting at `n × hop_size` of the stream, so
/// `start_frame` places a tile on the time axis no matter what order tiles
/// arrive in, and audio the analyser could not keep up with leaves a gap
/// rather than squashing time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SpectrogramTile {
    pub stream_id: String,
    /// Producer sequence number, increasing by one per tile of a stream.
    ///
    /// Named `tile_seq` rather than `seq` because polled event-log entries
    /// carry their own `seq` alongside a flattened payload; these two must
    /// never collide.
    pub tile_seq: u64,
    /// Absolute index of this tile's first frame within the stream.
    pub start_frame: u64,
    pub frame_count: u32,
    /// Frame-major decibel magnitudes: each frame's `bin_count` values from
    /// lowest frequency to highest, one frame after another. Always finite;
    /// silence is `floor_db`.
    pub magnitudes_db: Vec<f32>,
}

impl SpectrogramTile {
    /// Whether this tile's shape agrees with the stream it belongs to.
    pub fn matches(&self, meta: &SpectrogramStreamMeta) -> bool {
        self.stream_id == meta.stream_id
            && self.frame_count > 0
            && self.frame_count <= meta.limits.max_frames_per_tile
            && self.magnitudes_db.len()
                == self.frame_count as usize * meta.frequency.bin_count as usize
    }
}
