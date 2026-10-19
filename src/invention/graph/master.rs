//! Observers of the mixed master output.
//!
//! The audio thread hands each finished block to [`MasterObservers::observe`]
//! and nothing else; off-thread samplers read what the observers collected.
//! Adding an observer means adding it here, not threading another field
//! through every place a graph is built.

use std::sync::Arc;

#[cfg(test)]
mod level_tests;
mod levels;
#[cfg(test)]
mod loom_tests;

pub use levels::MasterLevels;
use levels::{BlockLevels, LevelMeter};

/// The atomics, lock and yield the level meter is built on. The loom models
/// compile the same source file against loom's versions instead.
mod sync {
    pub(super) use std::sync::atomic::{AtomicU64, Ordering};
    pub(super) use std::sync::Mutex;
    pub(super) use std::thread::yield_now;

    pub(super) use crate::audio_thread::debug_assert_control_thread;
}

/// Everything that watches the master output, shared between the audio-thread
/// graph and the runtime handle that samplers read through. Clones share state.
#[derive(Clone, Default)]
pub(crate) struct MasterObservers {
    /// Peak and RMS per meter window, drained into `MeterLevel` events.
    pub(crate) levels: Arc<LevelMeter>,
    /// Mono tap for spectrogram analysis. Only a live runtime has one: an
    /// offline render has no analyser to feed, so it carries no ring.
    #[cfg(feature = "spectrogram")]
    pub(crate) spectrum: Option<crate::spectrum::SpectrumTap>,
}

impl MasterObservers {
    /// Observers for a live runtime, which off-thread samplers can read.
    pub(crate) fn live() -> Self {
        Self {
            levels: Arc::default(),
            #[cfg(feature = "spectrogram")]
            spectrum: Some(crate::spectrum::SpectrumTap::new()),
        }
    }

    /// Audio thread: fold one mixed block, which ends at `end_frame` on the
    /// audio clock, into every observer.
    ///
    /// Lock-free and allocation-free. Peak and sum of squares are measured
    /// over buffers already in hand and handed to the level meter with two
    /// atomic adds; the spectrum tap is a single relaxed load unless an
    /// analyser is reading.
    #[inline]
    pub(crate) fn observe(&self, left: &[f32], right: &[f32], frames: usize, end_frame: u64) {
        let (left_peak, left_sum_of_squares) = channel_levels(&left[..frames]);
        let (right_peak, right_sum_of_squares) = channel_levels(&right[..frames]);
        let block = BlockLevels {
            left_peak,
            right_peak,
            left_sum_of_squares,
            right_sum_of_squares,
            frames: frames as u32,
        };
        self.levels.observe(&block, end_frame);

        #[cfg(feature = "spectrogram")]
        if let Some(spectrum) = &self.spectrum {
            spectrum.observe_block(left, right, frames);
        }
    }
}

/// Independent running sums per channel. A float sum may not be reordered,
/// so one running total is a chain of dependent adds; this many short chains
/// let the compiler vectorize it.
const LANES: usize = 8;

/// One channel's peak magnitude and sum of squares over a block. Two tight
/// loops over the block, which is already in cache: each vectorizes, where
/// one loop doing both measured over twice as slow. A NaN sample is skipped
/// by the peak and poisons the sum, so the window reports no RMS rather than
/// a wrong one.
fn channel_levels(samples: &[f32]) -> (f32, f32) {
    let peak = samples
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    let chunks = samples.chunks_exact(LANES);
    let tail: f32 = chunks
        .remainder()
        .iter()
        .map(|sample| sample * sample)
        .sum();
    let mut sums = [0.0f32; LANES];
    for chunk in chunks {
        for (sum, sample) in sums.iter_mut().zip(chunk) {
            *sum += sample * sample;
        }
    }
    (peak, sums.iter().sum::<f32>() + tail)
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
    ///
    /// Observing costs the same every block, so the cheapest of many short
    /// batches is that cost. A batch is well under a scheduler slice, so on a
    /// loaded machine some batches still run unpreempted, where one long
    /// wall-clock average would charge the observers for time spent waiting
    /// for a CPU.
    #[test]
    fn observing_a_block_costs_a_sliver_of_its_duration() {
        const FRAMES: usize = 128;
        const BATCHES: u32 = 200;
        const BLOCKS_PER_BATCH: u32 = 100;
        let budget = Duration::from_secs_f64(FRAMES as f64 / 48_000.0 / 100.0);
        let left: Vec<f32> = (0..FRAMES).map(|i| (i as f32 * 0.01).sin()).collect();
        let right: Vec<f32> = (0..FRAMES).map(|i| (i as f32 * 0.013).cos()).collect();
        let per_block = |observers: &MasterObservers| {
            (0..BATCHES)
                .map(|_| {
                    let start = Instant::now();
                    for _ in 0..BLOCKS_PER_BATCH {
                        observers.observe(black_box(&left), black_box(&right), FRAMES, 0);
                    }
                    start.elapsed() / BLOCKS_PER_BATCH
                })
                .min()
                .unwrap()
        };

        let observers = MasterObservers::live();
        let idle = per_block(&observers);
        eprintln!("master observers, idle: {idle:?} a block");
        assert!(idle < budget, "idle observers took {idle:?} a block");

        #[cfg(feature = "spectrogram")]
        {
            let tap = observers.spectrum.as_ref().expect("live observers tap");
            // A reader that never reads is the writer's steady state: it
            // never looks at readers, so it just keeps overwriting.
            let _reader = tap.reader();
            let reading = per_block(&observers);
            eprintln!("master observers, with a spectrum reader: {reading:?} a block");
            assert!(reading < budget, "with a reader, took {reading:?} a block");
        }
    }
}
