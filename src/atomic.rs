use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct AtomicF32 {
    bits: Arc<AtomicU32>,
}

impl AtomicF32 {
    pub(crate) fn new(value: f32) -> Self {
        Self {
            bits: Arc::new(AtomicU32::new(value.to_bits())),
        }
    }

    #[inline]
    pub(crate) fn load(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    #[inline]
    pub(crate) fn store(&self, value: f32) {
        self.bits.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// A shared stereo peak meter. Both handles reference the same atomics, so the
/// audio thread folds in block peaks while an off-thread sampler drains them.
///
/// There is exactly one audio-thread writer and one sampler; their races are
/// benign — a lost peak or a lost reset costs at most one ~100 ms meter tick,
/// never audio correctness. Crucially, the write path holds no locks and makes
/// no allocations, so it is safe on the audio callback (see the glitch-free
/// mandate in CLAUDE.md).
#[derive(Clone)]
pub(crate) struct StereoPeak {
    left: AtomicF32,
    right: AtomicF32,
}

impl Default for StereoPeak {
    fn default() -> Self {
        Self::new()
    }
}

impl StereoPeak {
    pub(crate) fn new() -> Self {
        Self {
            left: AtomicF32::new(0.0),
            right: AtomicF32::new(0.0),
        }
    }

    /// Audio thread: fold a block's peak magnitudes into the running maxima.
    #[inline]
    pub(crate) fn observe(&self, left_peak: f32, right_peak: f32) {
        if left_peak > self.left.load() {
            self.left.store(left_peak);
        }
        if right_peak > self.right.load() {
            self.right.store(right_peak);
        }
    }

    /// Off-thread sampler: read the peaks accumulated since the last call and
    /// reset them to zero, so each reading covers one sampling interval.
    pub(crate) fn drain(&self) -> (f32, f32) {
        let left = self.left.load();
        let right = self.right.load();
        self.left.store(0.0);
        self.right.store(0.0);
        (left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_peak_keeps_the_maximum_then_resets_on_drain() {
        let peak = StereoPeak::new();

        // Folds in keep the running maximum, ignoring smaller magnitudes.
        peak.observe(0.3, 0.9);
        peak.observe(0.7, 0.4);
        assert_eq!(peak.drain(), (0.7, 0.9));

        // Draining reset the accumulator, so a fresh window starts at zero.
        assert_eq!(peak.drain(), (0.0, 0.0));

        // Clones share the same atomics: an observe through one is visible
        // through the other (this is how the audio thread and sampler meet).
        let handle = peak.clone();
        handle.observe(0.5, 0.6);
        assert_eq!(peak.drain(), (0.5, 0.6));
    }
}
