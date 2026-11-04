//! Switches that crossfade: `bypass` and `slope`.
//!
//! Switching a filter in or out at once steps the output by the whole
//! difference between the dry and filtered signals, which is heard as a
//! click. Each switch is instead a share (0 to 1) of one signal mixed into
//! the other that moves over [`FADE_MS`] once the filter has been heard.
//! At rest the share is exactly 0 or 1 and the mix is skipped, so a bypassed
//! filter passes its input through bit for bit and a filter nobody switches
//! is unchanged.

use crate::dsp::LinearRamp;

/// How long a switch takes, in milliseconds.
pub const FADE_MS: f32 = 10.0;

const MS_PER_SECOND: f32 = 1000.0;

/// A share that moves between 0 and 1 over [`FADE_MS`].
#[derive(Clone, Copy, Debug)]
pub(super) struct Crossfade {
    share: LinearRamp,
}

impl Crossfade {
    /// A crossfade resting fully on (`on`) or off, at `sample_rate`.
    pub(super) fn new(sample_rate: u32, on: bool) -> Self {
        let frames = (sample_rate as f32 * FADE_MS / MS_PER_SECOND).round() as u32;
        Self {
            share: LinearRamp::new(share(on), frames),
        }
    }

    /// Heads on or off: fading when `heard`, at once before.
    pub(super) fn switch(&mut self, on: bool, heard: bool) {
        if heard {
            self.share.set_target(share(on));
        } else {
            self.share.snap(share(on));
        }
    }

    /// Whether the share is moving.
    #[inline(always)]
    pub(super) fn is_fading(&self) -> bool {
        self.share.is_gliding()
    }

    /// Whether the share rests fully on (only meaningful when not fading).
    #[inline(always)]
    pub(super) fn is_on(&self) -> bool {
        self.share.value() >= 1.0
    }

    /// Whether the share is, or is heading, on.
    #[inline(always)]
    pub(super) fn is_live(&self) -> bool {
        self.is_fading() || self.is_on()
    }

    /// Mixes `on` into `off` by the next frame's share: `off` exactly at 0,
    /// `on` exactly at 1.
    #[inline(always)]
    pub(super) fn mix(&mut self, off: f32, on: f32) -> f32 {
        if !self.share.is_gliding() {
            return if self.is_on() { on } else { off };
        }
        let share = self.share.next_value();
        if share >= 1.0 {
            on
        } else if share <= 0.0 {
            off
        } else {
            off + share * (on - off)
        }
    }
}

fn share(on: bool) -> f32 {
    if on {
        1.0
    } else {
        0.0
    }
}
