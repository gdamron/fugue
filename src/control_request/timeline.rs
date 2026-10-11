//! Clock timelines: the third layer of musical time.
//!
//! A clock keeps a continuous beat position: beats since its first gate,
//! or since its latest `reset`, fractional between beats. Beat `N` falls on
//! the first sample whose position reaches `N`, which is the sample its
//! `beat` gate rises on, so a request timed at a beat lands with the beat
//! a listener hears. Tempo changes keep the position continuous.
//!
//! The audio thread reads a clock's timeline between blocks and segments
//! (never while the clock processes) to place requests timed in beats: it
//! asks how many samples remain until a position at the current tempo,
//! and splits the block there.

/// The position of a clock that has output no sample since it was built
/// or reset: just before beat 0, so its next sample starts beat 0.
pub(crate) const BEFORE_START: f64 = -f64::MIN_POSITIVE;

/// A module's beat timeline, as the audio thread reads it.
pub(crate) trait Timeline {
    /// The position at the latest sample the module output, or
    /// [`BEFORE_START`] when it has output none since it was built or
    /// reset.
    fn position(&self) -> f64;

    /// The position the `samples`-th sample from now will have (the next
    /// sample is 1) if the tempo does not change before it.
    fn position_after(&self, samples: u64) -> f64;

    /// How many samples from now, counting the next as 1, until the
    /// position first reaches `beat` if the tempo does not change: `Some(1)`
    /// when the next sample does, `None` when it never will (a tempo that is
    /// not positive). Exact: it agrees sample for sample with the positions
    /// the module then outputs.
    fn samples_until(&self, beat: f64) -> Option<u64>;

    /// The beats begun before the module's latest reset, over every reset
    /// since it was built: a beat begins at its gate, and a reset begins
    /// none itself (its next sample begins beat 0). Added to the position
    /// it counts every beat begun, so a span counted in beats survives a
    /// reset: it jumps the count to the next whole beat, never back.
    fn beats_before(&self) -> u64;

    /// Holds the next sample at the position it is now predicted to have:
    /// a tempo change before it then takes effect from the sample after,
    /// so a request applied on a beat (a tempo change among them) does not
    /// move the beat it applies on. Until the next sample; no effect
    /// without a tempo change.
    fn latch(&mut self);
}

/// The least `k >= 1` with `position_after(k) >= beat`, given `estimate`,
/// a guess at it, for positions that never decrease as `k` grows; `None`
/// past `u64` range. A handful of evaluations when the estimate is close.
pub(crate) fn first_sample_reaching(
    position_after: impl Fn(u64) -> f64,
    beat: f64,
    estimate: f64,
) -> Option<u64> {
    if position_after(1) >= beat {
        return Some(1);
    }
    // Bracket it from the estimate, galloping: `lo` never reaches the beat
    // and `hi` does.
    let mut hi = if estimate.is_finite() {
        estimate.clamp(2.0, 2f64.powi(62)) as u64
    } else {
        2
    };
    let mut step = 1u64;
    let mut lo = if position_after(hi) >= beat {
        loop {
            let probe = hi.saturating_sub(step).max(1);
            if probe == 1 || position_after(probe) < beat {
                break probe;
            }
            hi = probe;
            step = step.saturating_mul(2);
        }
    } else {
        loop {
            let lo = hi;
            hi = hi.checked_add(step)?;
            if position_after(hi) >= beat {
                break lo;
            }
            step = step.saturating_mul(2);
        }
    };
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if position_after(mid) >= beat {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

#[cfg(test)]
mod tests {
    use super::first_sample_reaching;

    #[test]
    fn the_first_sample_reaching_a_beat_is_exact_from_any_estimate() {
        let spb = 22_050.0f64 * 60.0 / 97.0;
        let at = |k: u64| 0.25 + k as f64 / spb;
        for beat in [0.3, 1.0, 4.0, 17.5, 1000.0] {
            let exact = (1..).find(|&k| at(k) >= beat).unwrap();
            for estimate in [f64::NAN, 0.0, 1.0, exact as f64 - 3.0, exact as f64, 1e9] {
                assert_eq!(
                    first_sample_reaching(at, beat, estimate),
                    Some(exact),
                    "{beat} from {estimate}"
                );
            }
        }
        assert_eq!(first_sample_reaching(at, 0.0, 5.0), Some(1));
        assert_eq!(first_sample_reaching(|_| 0.0, 1.0, 1.0), None);
    }
}
