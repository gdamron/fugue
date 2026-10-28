//! The short fade that ends a sound at once without a click: a voice
//! stolen for a new note, a kit slot retriggered while it sounds, a note
//! released with a release of 0.
//!
//! A straight fade's slope jumps where it starts, a corner the ear hears
//! as a tick when the fade starts on a waveform's height. This one follows
//! smoothstep, flat at both ends, so the sound eases out with no corner.

/// How long a declick lasts: long enough not to click, short enough to
/// hear as the same instant.
pub(crate) const DECLICK_SECONDS: f32 = 0.003;

/// The per-frame step of a declick's progress at `sample_rate`: the
/// progress runs from 1 at the fade's start down to 0.
pub(crate) fn declick_step(sample_rate: f32) -> f32 {
    1.0 / (DECLICK_SECONDS * sample_rate).max(1.0)
}

/// A declick's gain at `progress` (1 as it starts, 0 once it ends):
/// smoothstep, `3p² - 2p³`, whose slope is zero at both ends.
#[inline]
pub(crate) fn declick_gain(progress: f32) -> f32 {
    let p = progress.clamp(0.0, 1.0);
    p * p * (3.0 - 2.0 * p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_declick_runs_from_full_to_silent_with_no_corner_at_either_end() {
        assert_eq!(declick_gain(1.0), 1.0);
        assert_eq!(declick_gain(0.0), 0.0);
        assert_eq!(declick_gain(0.5), 0.5);
        // A straight fade drops by a whole step on its first and last
        // frames; this one by a small fraction of it.
        let step = declick_step(48_000.0);
        assert!(1.0 - declick_gain(1.0 - step) < 0.05 * step);
        assert!(declick_gain(step) < 0.05 * step);
        let mut last = 1.0;
        let mut progress = 1.0;
        while progress > 0.0 {
            progress -= step;
            let gain = declick_gain(progress);
            assert!(gain <= last && last - gain <= 1.5 * step + 1e-6);
            last = gain;
        }
    }
}
