//! Ramps in beats ([`BeatTime::ramp`]): a request carrying one applies
//! where its clock reaches its beat, as any request timed in beats does,
//! and from there glides its control linearly to the request's value,
//! arriving exactly on the sample the clock reaches the ramp's end.
//!
//! Positions are counted as spans are: a reset of the clock while the
//! ramp runs jumps the count to the next whole beat, so the ramp carries on
//! from there rather than starting over. Values are interpolated in `f64`
//! and kept between the ramp's endpoints, so a ramp between finite values
//! writes only finite values (FUG-302).
//!
//! [`BeatTime::ramp`]: super::BeatTime::ramp

use super::timeline::Timeline;

/// A ramp's course on its clock's timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BeatRamp {
    from: f32,
    to: f32,
    /// Where it starts and ends, in its clock's positions as of when the
    /// clock had begun `beats_before` beats before its latest reset.
    start: f64,
    end: f64,
    beats_before: u64,
}

impl BeatRamp {
    /// From `from` at position `start` to `to` `beats` later, as of a clock
    /// that has begun `beats_before` beats before its latest reset.
    pub(crate) fn new(from: f32, to: f32, start: f64, beats: f64, beats_before: u64) -> Self {
        Self {
            from,
            to,
            start,
            end: start + beats,
            beats_before,
        }
    }

    /// The beats `timeline` has begun since the ramp's count, over every
    /// reset since it started: each jumps the count to the next whole beat.
    fn resets(&self, timeline: &dyn Timeline) -> f64 {
        timeline.beats_before().saturating_sub(self.beats_before) as f64
    }

    /// What it writes at the clock's next sample, and whether that is its
    /// last write (it has arrived); `None` when the clock's position is no
    /// number.
    pub(crate) fn next(&self, timeline: &dyn Timeline) -> Option<(f32, bool)> {
        let position = timeline.position_after(1) + self.resets(timeline);
        if position.is_nan() {
            return None;
        }
        if position >= self.end {
            return Some((self.to, true));
        }
        let progress = (position - self.start) / (self.end - self.start);
        Some((ramp_value(self.from, self.to, progress), false))
    }

    /// Samples until the clock reaches the ramp's end, at its tempo now.
    pub(crate) fn samples_to_end(&self, timeline: &dyn Timeline) -> Option<u64> {
        timeline.samples_until(self.end - self.resets(timeline))
    }
}

/// `from + (to - from) * progress`, for `progress` clamped to `[0, 1]`:
/// computed in `f64`, where `to - from` cannot overflow, and kept between
/// the endpoints so rounding never overshoots `to`. A NaN `progress`
/// yields `from`.
pub(crate) fn ramp_value(from: f32, to: f32, progress: f64) -> f32 {
    // NaN fails the comparison and `max` takes 0 over it.
    let progress = if progress >= 1.0 {
        1.0
    } else {
        progress.max(0.0)
    };
    let (from_wide, to_wide) = (f64::from(from), f64::from(to));
    let value = (from_wide + (to_wide - from_wide) * progress) as f32;
    value.max(from.min(to)).min(from.max(to))
}
