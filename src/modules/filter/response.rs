//! One response as it plays: its type, its stages, its slope fade and its
//! EQ design.
//!
//! Switching `filter_type` between two samples jumps from one response's
//! output to another's, which is heard as a click on anything bright. The
//! filter instead plays the outgoing response on a copy of the playing one
//! (same state, so it carries on exactly as it would have) while the new
//! response fades in over [`super::FADE_MS`]. Both hear the same input,
//! cutoff and glides; once the fade is done only the new one runs.

use super::fade::Crossfade;
use super::shape::{Coefficients, EqDesign, Stage};
use super::FilterType;

/// The damping of the second stage a `24db` slope adds: Butterworth, with
/// no peak of its own, so `resonance` shapes one peak rather than squaring
/// it (two resonant stages in series would ring far louder than either).
pub(super) const SECOND_STAGE_DAMPING: f32 = std::f32::consts::SQRT_2;

/// What every response hears on one frame.
pub(super) struct Frame {
    pub(super) input: f32,
    pub(super) cutoff: f32,
    pub(super) boost: f32,
    pub(super) q: f32,
}

/// The lowpass, highpass and bandpass coefficients of the first stage and
/// of the `24db` slope's second, shared by every classic response.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Classic {
    pub(super) first: Coefficients,
    pub(super) second: Coefficients,
}

/// A response and the state it plays from.
#[derive(Clone, Copy, Debug)]
pub(super) struct Response {
    pub(super) filter_type: FilterType,
    pub(super) stage: Stage,
    pub(super) second_stage: Stage,
    /// How much of the second stage is heard (`slope`).
    pub(super) steep: Crossfade,
    pub(super) eq: EqDesign,
}

impl Response {
    /// A lowpass at rest, at `sample_rate`.
    pub(super) fn new(sample_rate: u32) -> Self {
        Self {
            filter_type: FilterType::LowPass,
            stage: Stage::default(),
            second_stage: Stage::default(),
            steep: Crossfade::new(sample_rate, false),
            eq: EqDesign::new(sample_rate),
        }
    }

    /// Filters one frame.
    #[inline(always)]
    pub(super) fn sample(&mut self, frame: &Frame, classic: &Classic) -> f32 {
        if self.filter_type.is_eq() {
            self.eq_sample(frame)
        } else {
            self.classic_sample(classic, frame.input)
        }
    }

    /// The lowpass, highpass or bandpass response, through the trapezoidal
    /// integrated SVF (Andrew Simper / Cytomic). Unconditionally stable at
    /// all frequencies.
    #[inline(always)]
    fn classic_sample(&mut self, classic: &Classic, input: f32) -> f32 {
        let filter_type = self.filter_type;
        let first = self.stage.classic(&classic.first, filter_type, input);
        if !self.steep.is_live() {
            return first;
        }
        let second = self
            .second_stage
            .classic(&classic.second, filter_type, first);
        // The Butterworth bandpass tap peaks at 1/k; k·bandpass peaks at 1,
        // so the steep band is narrower but no quieter.
        let second = if filter_type == FilterType::BandPass {
            second * SECOND_STAGE_DAMPING
        } else {
            second
        };
        self.steep.mix(first, second)
    }

    /// An EQ response.
    #[inline(always)]
    fn eq_sample(&mut self, frame: &Frame) -> f32 {
        (self.eq).design(self.filter_type, frame.cutoff, frame.boost, frame.q);
        let coefficients = &self.eq.coefficients;
        let (bandpass, lowpass) = self.stage.tick(coefficients, frame.input);
        coefficients.mix(frame.input, bandpass, lowpass)
    }

    pub(super) fn reset(&mut self) {
        self.stage.reset();
        self.second_stage.reset();
    }
}
