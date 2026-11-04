//! Cutoff and resonance writes that glide instead of jumping.
//!
//! A filter whose cutoff jumps between two samples steps its output: opening
//! a dark lowpass at once lets the whole signal through on the next sample,
//! and the step is heard as a click. Spread over a few milliseconds the same
//! change is heard as a fast sweep. Cutoff glides in octaves (log2 Hz), so a
//! wide jump spends as long crossing each octave as any other; resonance
//! glides linearly.
//!
//! Only control writes glide. The audio-rate `cutoff`, `cutoff_mod` and
//! `resonance` inputs stay per-sample, so envelope sweeps and filter FM are
//! untouched. Writes made before the filter is first heard (its config, a
//! development's starting values) land at once.

use crate::dsp::LinearRamp;

/// How long a cutoff or resonance write takes to arrive, in milliseconds.
pub const GLIDE_MS: f32 = 10.0;

const MS_PER_SECOND: f32 = 1000.0;

/// The glides of a filter's cutoff (in log2 Hz) and resonance.
#[derive(Clone, Copy, Debug)]
pub(super) struct ControlGlide {
    cutoff_octaves: LinearRamp,
    resonance: LinearRamp,
    heard: bool,
}

impl ControlGlide {
    /// Glides resting at `cutoff` Hz and `resonance`, at `sample_rate`.
    pub(super) fn new(sample_rate: u32, cutoff: f32, resonance: f32) -> Self {
        let frames = (sample_rate as f32 * GLIDE_MS / MS_PER_SECOND).round() as u32;
        Self {
            cutoff_octaves: LinearRamp::new(cutoff.log2(), frames),
            resonance: LinearRamp::new(resonance, frames),
            heard: false,
        }
    }

    /// Moves the cutoff to `hz`: gliding once the filter has been heard, at
    /// once before that.
    pub(super) fn cutoff_to(&mut self, hz: f32) {
        if self.heard {
            self.cutoff_octaves.set_target(hz.log2());
        } else {
            self.cutoff_octaves.snap(hz.log2());
        }
    }

    /// Moves the resonance to `resonance`, as [`Self::cutoff_to`] does.
    pub(super) fn resonance_to(&mut self, resonance: f32) {
        if self.heard {
            self.resonance.set_target(resonance);
        } else {
            self.resonance.snap(resonance);
        }
    }

    /// Marks the filter heard: later writes glide.
    pub(super) fn heard(&mut self) {
        self.heard = true;
    }

    /// The cutoff for the next frame. `resting` is the control's value in Hz,
    /// returned exactly whenever no glide is running (and on a glide's last
    /// frame), so a filter nobody moves is unchanged.
    #[inline(always)]
    pub(super) fn next_cutoff(&mut self, resting: f32) -> f32 {
        if !self.cutoff_octaves.is_gliding() {
            return resting;
        }
        let octaves = self.cutoff_octaves.next_value();
        if self.cutoff_octaves.is_gliding() {
            octaves.exp2()
        } else {
            resting
        }
    }

    /// The resonance for the next frame (exactly `resting` when not gliding;
    /// a glide's last frame is its target).
    #[inline(always)]
    pub(super) fn next_resonance(&mut self, resting: f32) -> f32 {
        if self.resonance.is_gliding() {
            self.resonance.next_value()
        } else {
            resting
        }
    }
}
