//! Observers of the mixed master output.
//!
//! The audio thread hands each finished block to [`MasterObservers::observe`]
//! and nothing else; off-thread samplers read what the observers collected.
//! Adding an observer means adding it here, not threading another field
//! through every place a graph is built.

use crate::atomic::StereoPeak;

/// Everything that watches the master output, shared between the audio-thread
/// graph and the runtime handle that samplers read through. Clones share state.
#[derive(Clone, Default)]
pub(crate) struct MasterObservers {
    /// Peak level, drained into `MeterLevel` events (FUG-239 #5).
    pub(crate) peak: StereoPeak,
    /// Mono tap for spectrogram analysis. Only a live runtime has one: an
    /// offline render has no analyser to feed, so it carries no ring.
    #[cfg(feature = "spectrogram")]
    pub(crate) spectrum: Option<crate::spectrum::SpectrumTap>,
}

impl MasterObservers {
    /// Observers for a live runtime, which off-thread samplers can read.
    pub(crate) fn live() -> Self {
        Self {
            peak: StereoPeak::new(),
            #[cfg(feature = "spectrogram")]
            spectrum: Some(crate::spectrum::SpectrumTap::new()),
        }
    }

    /// Audio thread: fold one mixed block into every observer.
    ///
    /// Lock-free and allocation-free. The peak is one pass over buffers
    /// already in hand; the spectrum tap is a single relaxed load unless an
    /// analyser is reading.
    #[inline]
    pub(crate) fn observe(&self, left: &[f32], right: &[f32], frames: usize) {
        let mut left_peak = 0.0f32;
        let mut right_peak = 0.0f32;
        for i in 0..frames {
            left_peak = left_peak.max(left[i].abs());
            right_peak = right_peak.max(right[i].abs());
        }
        self.peak.observe(left_peak, right_peak);

        #[cfg(feature = "spectrogram")]
        if let Some(spectrum) = &self.spectrum {
            spectrum.observe_block(left, right, frames);
        }
    }
}
