//! The output levels of an offline render, read the way a live meter reads
//! them: peak before the device clamp, so overs show, with the samples the
//! clamp cut and the non-finite samples the sinks silenced.

use serde::Serialize;

use super::RenderEngine;

/// What an offline render's output held since the last
/// [`RenderEngine::take_levels`] (or since the piece loaded).
///
/// Peaks and RMS are linear amplitude, 1.0 being full scale, measured on the
/// mix before the device clamp, so a peak above 1.0 is an over. The samples
/// rendered are clamped: `left_overs` and `right_overs` count those the
/// clamp cut, beyond ±1.0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct OfflineLevels {
    /// Largest absolute sample of the mix, left channel.
    pub left_peak: f32,
    /// Largest absolute sample of the mix, right channel.
    pub right_peak: f32,
    /// RMS of the mix before the clamp, left channel; absent with no frames
    /// or a sum of squares that is not finite.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_rms: Option<f32>,
    /// RMS of the mix before the clamp, right channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_rms: Option<f32>,
    /// Frames rendered.
    pub frames: u64,
    /// Left samples beyond ±1.0, which the clamp cut.
    pub left_overs: u64,
    /// Right samples beyond ±1.0, which the clamp cut.
    pub right_overs: u64,
    /// Non-finite samples (NaN or ±inf) the sinks replaced with silence. A
    /// sink rebuilt or removed during the stretch takes its count with it,
    /// so that stretch can read low.
    pub non_finite_samples: u64,
}

/// Counts of what the device clamp cut, kept by the render between reads.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct ClampCounts {
    overs: [u64; 2],
    /// The sinks' running total of non-finite samples at the last read.
    non_finite_read: u64,
}

impl ClampCounts {
    /// Counts one frame of the mix, before it is clamped. A sample the
    /// clamp silences rather than cuts (an infinite sum of finite sinks) is
    /// no over. Branch-free: a few compares and adds a frame, no allocation.
    #[inline]
    pub(super) fn count(&mut self, left: f32, right: f32) {
        self.overs[0] += u64::from(left.is_finite() && left.abs() > 1.0);
        self.overs[1] += u64::from(right.is_finite() && right.abs() > 1.0);
    }
}

impl RenderEngine {
    /// The output's levels since the last call (or since the piece loaded),
    /// after which the next stretch starts; `None` with nothing loaded.
    /// Read between renders, never during one.
    pub fn take_levels(&mut self) -> Option<OfflineLevels> {
        let graph = self.graph.as_ref()?;
        let levels = graph.lock().unwrap().master.levels.drain(self.sample_rate);
        let counts = std::mem::take(&mut self.clamp_counts);
        // A running total; it drops back when a sink is rebuilt.
        let total = levels.non_finite_samples;
        self.clamp_counts.non_finite_read = total;
        Some(OfflineLevels {
            left_peak: levels.left_peak,
            right_peak: levels.right_peak,
            left_rms: levels.left_rms,
            right_rms: levels.right_rms,
            frames: levels.frames,
            left_overs: counts.overs[0],
            right_overs: counts.overs[1],
            non_finite_samples: total.checked_sub(counts.non_finite_read).unwrap_or(total),
        })
    }
}
