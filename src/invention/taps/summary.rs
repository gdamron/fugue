//! What a view reads from a tap: per-channel min/max bins over a stated
//! span of samples, the waveform component's format.
//!
//! The audio thread summarises each [`TAP_BIN`] samples; a read merges
//! those into bins of the width the view asks for (a whole number of
//! them). What it returns is contiguous: a gap (a full ring, or a module
//! that was not running for a while) drops what came before it, and the
//! samples skipped since the last read are counted in
//! [`OutputBins::lost`].

use super::OutputTap;
use crate::invention::graph::taps::{TapBin, TAP_BIN};

/// Bins per channel one read returns at most: a view showing more asks for
/// wider bins. Older bins past it are skipped and counted lost.
pub const MAX_TAP_BINS: usize = 4096;

/// Samples a bin covers at most, so a merge stays a bounded loop.
const MAX_SAMPLES_PER_BIN: u32 = 1 << 20;

/// One read of a tap: what each tapped output did since the last read.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputBins {
    /// The module tapped.
    pub module_id: String,
    /// The outputs read, one per channel, in channel order.
    pub ports: Vec<String>,
    /// Samples a second, to put [`Self::origin_frame`] and the bins'
    /// width in seconds.
    pub sample_rate: u32,
    /// Samples each bin covers: the width asked for, rounded up to a whole
    /// number of the audio thread's bins.
    pub samples_per_bin: u32,
    /// The first bin's first sample, counted from zero as the invention
    /// started playing (the clock [`crate::NoteTime::frame`] counts on).
    pub origin_frame: u64,
    /// Per channel, in [`Self::ports`] order. Values are the module's own
    /// output, unscaled and unclipped: for audio, 1.0 is full scale. A bin
    /// over nothing but NaN reads NaN.
    pub channels: Vec<ChannelBins>,
    /// Samples between the end of the last read and [`Self::origin_frame`]
    /// that no bin covers: dropped by a full ring, skipped past
    /// [`MAX_TAP_BINS`], or played while the module was not running.
    pub lost: u64,
    /// Whether the tap is still open: false once an edit removed or
    /// rebuilt the module, or the invention stopped. A closed tap reads
    /// nothing more.
    pub open: bool,
}

/// One channel's bins, oldest first: `min[n]` and `max[n]` bound bin `n`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelBins {
    pub min: Vec<f32>,
    pub max: Vec<f32>,
}

impl OutputBins {
    /// [`Self::origin_frame`] in seconds.
    pub fn origin_seconds(&self) -> f64 {
        self.origin_frame as f64 / f64::from(self.sample_rate)
    }

    /// Bins per channel.
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, |channel| channel.min.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl OutputTap {
    /// Reads every bin the audio thread summarised since the last read,
    /// merged into bins `samples_per_bin` wide (rounded up to a multiple of
    /// [`TAP_BIN`]), at most [`MAX_TAP_BINS`] of them, the newest. A
    /// remainder too short for a whole bin waits for the next read.
    /// Control thread; never blocks the audio thread.
    pub fn read(&mut self, samples_per_bin: u32) -> OutputBins {
        let samples_per_bin = samples_per_bin.clamp(1, MAX_SAMPLES_PER_BIN);
        let factor = (samples_per_bin as usize).div_ceil(TAP_BIN);
        let open = self.take_bins();
        if !open {
            // What it kept back is a closed module's too.
            self.pending.clear();
        }
        let pending = &mut self.pending;
        // Keep the latest contiguous run.
        let contiguous = |pair: &[TapBin]| pair[1].start == pair[0].start + TAP_BIN as u64;
        if let Some(gap) = pending.windows(2).rposition(|pair| !contiguous(pair)) {
            pending.drain(..=gap);
        }
        let whole = pending.len() / factor;
        let skipped = whole.saturating_sub(MAX_TAP_BINS);
        let groups = pending[skipped * factor..whole * factor].chunks_exact(factor);
        let origin_frame =
            (pending.get(skipped * factor)).map_or(self.next.unwrap_or(0), |bin| bin.start);
        let mut channels = vec![ChannelBins::default(); self.ports.len()];
        for group in groups {
            for (c, channel) in channels.iter_mut().enumerate() {
                let (min, max) = group
                    .iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(min, max), bin| {
                        (min.min(bin.min[c]), max.max(bin.max[c]))
                    });
                let (min, max) = if min > max {
                    (f32::NAN, f32::NAN)
                } else {
                    (min, max)
                };
                channel.min.push(min);
                channel.max.push(max);
            }
        }
        let read = whole - skipped;
        pending.drain(..whole * factor);
        let lost = match self.next {
            Some(next) if read > 0 => origin_frame.saturating_sub(next),
            _ => 0,
        };
        if read > 0 {
            self.next = Some(origin_frame + (read * factor * TAP_BIN) as u64);
        }
        OutputBins {
            module_id: self.module_id.clone(),
            ports: self.ports.clone(),
            sample_rate: self.sample_rate,
            samples_per_bin: (factor * TAP_BIN) as u32,
            origin_frame,
            channels,
            lost,
            open,
        }
    }
}
