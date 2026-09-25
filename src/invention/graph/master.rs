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

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;
    use std::time::{Duration, Instant};

    /// A tripwire for what the master observers cost the audio thread, idle
    /// and with a spectrum reader attached. The budget is a hundredth of the
    /// block's real-time duration: hundreds of times what an optimised build
    /// spends, and still far below anything a listener could hear.
    #[test]
    fn observing_a_block_costs_a_sliver_of_its_duration() {
        const FRAMES: usize = 128;
        const BLOCKS: u32 = 20_000;
        let budget = Duration::from_secs_f64(FRAMES as f64 / 48_000.0 / 100.0);
        let left: Vec<f32> = (0..FRAMES).map(|i| (i as f32 * 0.01).sin()).collect();
        let right: Vec<f32> = (0..FRAMES).map(|i| (i as f32 * 0.013).cos()).collect();
        let per_block = |observers: &MasterObservers| {
            let start = Instant::now();
            for _ in 0..BLOCKS {
                observers.observe(black_box(&left), black_box(&right), FRAMES);
            }
            start.elapsed() / BLOCKS
        };

        let observers = MasterObservers::live();
        let idle = per_block(&observers);
        assert!(idle < budget, "idle observers took {idle:?} a block");

        #[cfg(feature = "spectrogram")]
        {
            let tap = observers.spectrum.as_ref().expect("live observers tap");
            // A reader that never reads is the writer's steady state: it
            // never looks at readers, so it just keeps overwriting.
            let _reader = tap.reader();
            let reading = per_block(&observers);
            assert!(reading < budget, "with a reader, took {reading:?} a block");
        }
    }
}
