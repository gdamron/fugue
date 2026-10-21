//! The reverb's delay network tuning, and the crossfade that carries the
//! network from one tuning to the next.
//!
//! A new room size moves every tap and FDN read to a different sample of the
//! delay buffers. Reading the new offsets at once steps the waveform, which
//! is heard as a click on a step and as zipper noise under automation.
//! Instead, the reverb keeps reading the old offsets alongside the new ones
//! and blends from one to the other over [`FADE_SECONDS`].

use super::{
    compute_alpha, decay_to_rt60, room_size_to_meters, Reverb, FDN_ORDER, FDN_RATIOS,
    SPEED_OF_SOUND, TAP_FRACTIONS, TAP_OFFSET,
};

/// How long one crossfade between tunings takes. Long enough that the blend
/// of two unrelated tails is heard as a change of room rather than a click,
/// short enough that the room follows automation closely.
const FADE_SECONDS: f32 = 0.03;

/// The delay network's lengths and gains for one room size, decay and
/// freeze state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Tuning {
    pub(super) fdn_lens: [usize; FDN_ORDER],
    pub(super) fdn_gains: [f32; FDN_ORDER],
    pub(super) tap_positions: [usize; FDN_ORDER],
    pub(super) tap_gains: [f32; FDN_ORDER],
}

/// A crossfade from one [`Tuning`] to another, owned by the audio thread.
///
/// The blend's two weights always sum to one (the shape is a smoothstep, not
/// equal-power). The FDN reads sit inside the feedback loop, and weights that
/// summed to more than one (equal-power peaks at √2) would lift the loop gain
/// above unity mid-fade, which a long decay or freeze would hear as a swell.
/// A sum of one keeps the loop no louder than either tuning alone; the price
/// is a dip in the middle of the fade, where the two tails are unrelated: at
/// most 3 dB for one pass, but it compounds round the loop, so a frozen tail
/// fades a little with each change of room. The smoothstep starts and ends
/// with zero slope, so the fade itself adds no corner to the waveform.
///
/// A new tuning arriving mid-fade waits for the fade to finish rather than
/// cutting it short, which would jump. Only the latest waiting tuning is
/// kept, so under automation the network steps toward the newest value one
/// fade at a time, never more than two fades behind.
pub(super) struct Retuning {
    from: Tuning,
    to: Tuning,
    waiting: Option<Tuning>,
    /// Samples left in the fade under way: 0 once settled on `to`.
    remaining: u32,
    /// Samples in one fade, and its reciprocal.
    frames: u32,
    step: f32,
}

impl Retuning {
    /// Settled on `tuning`, with fades lasting [`FADE_SECONDS`].
    pub(super) fn settled(tuning: Tuning, sample_rate: u32) -> Self {
        let frames = ((FADE_SECONDS * sample_rate as f32).round() as u32).max(1);
        Self {
            from: tuning,
            to: tuning,
            waiting: None,
            remaining: 0,
            frames,
            step: 1.0 / frames as f32,
        }
    }

    /// Fades to `tuning`, after any fade under way.
    pub(super) fn retarget(&mut self, tuning: Tuning) {
        if self.remaining > 0 {
            self.waiting = Some(tuning);
        } else if tuning != self.to {
            self.start(tuning);
        }
    }

    fn start(&mut self, tuning: Tuning) {
        self.from = self.to;
        self.to = tuning;
        self.remaining = self.frames;
    }

    /// Advances one sample and returns the weight of [`Self::to`] against
    /// [`Self::from`] for it: 1 once settled, so `from` need not be read.
    #[inline]
    pub(super) fn next_weight(&mut self) -> f32 {
        if self.remaining == 0 {
            return 1.0;
        }
        self.remaining -= 1;
        if self.remaining == 0 {
            self.from = self.to;
            match self.waiting.take() {
                // This sample is all `to`, which is where the next fade
                // starts: weight 0 of the new target.
                Some(next) if next != self.to => self.start(next),
                _ => return 1.0,
            }
        }
        let t = 1.0 - self.remaining as f32 * self.step;
        t * t * (3.0 - 2.0 * t)
    }

    /// The tuning being faded from.
    pub(super) fn from(&self) -> &Tuning {
        &self.from
    }

    /// The tuning being faded to, or settled on.
    pub(super) fn to(&self) -> &Tuning {
        &self.to
    }
}

/// Blends a reading under the old tuning into one under the new by weight
/// `s` of the new, skipping the old read once settled.
#[inline]
pub(super) fn blend(s: f32, old: impl FnOnce() -> f32, new: f32) -> f32 {
    if s < 1.0 {
        let old = old();
        old + s * (new - old)
    } else {
        new
    }
}

impl Reverb {
    /// The delay network's tuning for the current controls.
    pub(super) fn tuning(&self) -> Tuning {
        let frozen = self.freeze;
        let room_m = room_size_to_meters(self.room_size);
        let largest_delay = ((self.sample_rate as f32 * room_m / SPEED_OF_SOUND) as usize).max(1);

        let rt60 = decay_to_rt60(self.decay);
        let alpha = compute_alpha(rt60, self.sample_rate);

        // FDN lengths (clamped to allocated max)
        let fdn_lens: [usize; FDN_ORDER] = std::array::from_fn(|i| {
            ((largest_delay as f32 * FDN_RATIOS[i]) as usize)
                .max(1)
                .min(self.max_fdn_lengths[i])
        });

        // FDN gains from alpha
        let fdn_gains: [f32; FDN_ORDER] = if frozen {
            [-1.0; FDN_ORDER]
        } else {
            std::array::from_fn(|i| -(alpha.powi(fdn_lens[i] as i32)))
        };

        // Tap positions and gains
        let tap_positions: [usize; FDN_ORDER] = std::array::from_fn(|i| {
            let pos = (TAP_FRACTIONS[i] * largest_delay as f32) as usize + TAP_OFFSET;
            pos.min(self.max_tap_delay_len - 1)
        });
        let tap_gains: [f32; FDN_ORDER] = if frozen {
            [0.0; FDN_ORDER]
        } else {
            std::array::from_fn(|i| alpha.powi(tap_positions[i] as i32))
        };

        Tuning {
            fdn_lens,
            fdn_gains,
            tap_positions,
            tap_gains,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning(len: usize) -> Tuning {
        Tuning {
            fdn_lens: [len; FDN_ORDER],
            ..Tuning::default()
        }
    }

    /// The weights `retuning` gives over `frames` samples.
    fn weights(retuning: &mut Retuning, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| retuning.next_weight()).collect()
    }

    #[test]
    fn settled_reads_only_the_new_tuning() {
        let mut retuning = Retuning::settled(tuning(10), 1000);
        assert_eq!(retuning.next_weight(), 1.0);
        retuning.retarget(tuning(10));
        assert_eq!(retuning.next_weight(), 1.0, "the same tuning is no fade");
    }

    #[test]
    fn a_fade_rises_smoothly_from_the_old_tuning_to_the_new() {
        let mut retuning = Retuning::settled(tuning(10), 1000);
        retuning.retarget(tuning(20));
        assert_eq!(retuning.from(), &tuning(10));
        assert_eq!(retuning.to(), &tuning(20));
        // 30 ms at 1 kHz.
        let weights = weights(&mut retuning, 30);
        assert!(weights[0] > 0.0 && weights[0] < 0.01, "{}", weights[0]);
        assert!(weights.windows(2).all(|w| w[1] > w[0]), "{weights:?}");
        assert_eq!(weights[29], 1.0);
        assert_eq!(retuning.from(), &tuning(20));
    }

    #[test]
    fn a_tuning_arriving_mid_fade_waits_and_only_the_latest_is_kept() {
        let mut retuning = Retuning::settled(tuning(10), 1000);
        retuning.retarget(tuning(20));
        for _ in 0..10 {
            retuning.next_weight();
        }
        retuning.retarget(tuning(30));
        retuning.retarget(tuning(40));
        assert_eq!(retuning.to(), &tuning(20), "the fade under way finishes");
        let rest = weights(&mut retuning, 20);
        // The sample that finishes the first fade starts the next one from
        // weight 0 of the new target, so the output carries on unbroken.
        assert_eq!(rest[..19].iter().filter(|&&w| w >= 1.0).count(), 0);
        assert_eq!(rest[19], 0.0);
        assert_eq!(retuning.from(), &tuning(20));
        assert_eq!(retuning.to(), &tuning(40));
        assert_eq!(weights(&mut retuning, 30)[29], 1.0);
        assert_eq!(retuning.from(), &tuning(40));
    }
}
