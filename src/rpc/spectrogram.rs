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
/// Named on every stream so encodings can be added without a schema version
/// bump: a client reads this and picks a decoder, or reports the stream as
/// unsupported instead of misreading it. An encoding this build does not know
/// still parses, as [`Unsupported`](Self::Unsupported).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpectrogramEncoding {
    /// `magnitudes` is a base64 string (standard alphabet, padded) of one byte
    /// per value, linear in decibels: 0 is `floor_db`, 255 is `ceiling_db`,
    /// and level `q` decodes to `floor_db + q / 255 × (ceiling_db − floor_db)`.
    /// Levels outside the range are clamped to it. About a fifth the size
    /// of `f32_json`, at a resolution (0.4 dB over a 100 dB range) finer than
    /// any colour map shows.
    U8Base64,
    /// `magnitudes` is a JSON array of decibel values, rounded to 0.1 dB.
    /// Simple to read by eye, and the size to expect for it.
    F32Json,
    /// An encoding this build does not know. Never produced; a client that
    /// reads it reports the stream unsupported and ignores its tiles.
    #[serde(other)]
    Unsupported,
}

/// How bins are spaced along the frequency axis. This is how the producer
/// laid its bins out, not how a view draws them: a view may still draw a
/// linearly spaced stream on a log axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpectrogramBinSpacing {
    /// Bin `k` is centred on `min_hz + k × bin_hz`.
    Linear,
    /// A spacing this build does not know. Never produced; a client that
    /// reads it reports the stream unsupported.
    #[serde(other)]
    Unsupported,
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
///
/// `spacing` is named for the same reason as a stream's encoding: a reduced
/// axis (log-spaced bands, say) can be added later as a new spacing rather
/// than a schema version bump.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
pub struct SpectrogramFrequencyAxis {
    pub spacing: SpectrogramBinSpacing,
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

/// The magnitudes of a tile, encoded as its stream's
/// [`encoding`](SpectrogramStreamMeta::encoding) says.
///
/// Untagged on the wire, and named here by JSON shape rather than by
/// encoding: the stream's metadata is what says how to read them, and a
/// future text encoding would arrive as the same shape as `u8_base64`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "rpc-schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum SpectrogramMagnitudes {
    /// A string, as [`SpectrogramEncoding::U8Base64`] sends.
    Text(String),
    /// An array of numbers, as [`SpectrogramEncoding::F32Json`] sends.
    Numbers(Vec<f32>),
}

impl SpectrogramMagnitudes {
    /// Whether these hold exactly `values` magnitudes in `encoding`.
    ///
    /// For base64 that means the right length and the right padding, which
    /// together fix the byte count without decoding anything.
    pub fn fit(&self, encoding: SpectrogramEncoding, values: usize) -> bool {
        match (self, encoding) {
            (Self::Numbers(numbers), SpectrogramEncoding::F32Json) => numbers.len() == values,
            (Self::Text(text), SpectrogramEncoding::U8Base64) => {
                let Some(expected) = values.div_ceil(3).checked_mul(4) else {
                    return false;
                };
                let padding = (3 - values % 3) % 3;
                let padded = text.bytes().rev().take_while(|byte| *byte == b'=').count();
                text.len() == expected && padded == padding
            }
            _ => false,
        }
    }
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
    /// Frame-major magnitudes: each frame's `bin_count` values from lowest
    /// frequency to highest, one frame after another, encoded as the stream
    /// declares. Always finite; silence is `floor_db`.
    pub magnitudes: SpectrogramMagnitudes,
}

impl SpectrogramTile {
    /// Whether this tile's shape and encoding agree with the stream it
    /// belongs to.
    pub fn matches(&self, meta: &SpectrogramStreamMeta) -> bool {
        if self.stream_id != meta.stream_id
            || self.frame_count == 0
            || self.frame_count > meta.limits.max_frames_per_tile
        {
            return false;
        }
        // Checked, so a malformed tile or stream cannot overflow a 32-bit
        // client into a panic.
        let Some(values) =
            (self.frame_count as usize).checked_mul(meta.frequency.bin_count as usize)
        else {
            return false;
        };
        self.magnitudes.fit(meta.encoding, values)
    }
}
