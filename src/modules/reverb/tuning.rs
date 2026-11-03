//! The reverb's delay network tuning, and the crossfade that carries the
//! network from one tuning to the next.
//!
//! A new room size moves every diffuser and feedback-line read to a
//! different sample of the delay buffers. Reading the new offsets at once
//! steps the waveform, which is heard as a click on a step and as zipper
//! noise under automation.
//! Instead, the reverb keeps reading the old offsets alongside the new ones
//! and blends from one to the other over [`FADE_SECONDS`].

use super::network::{CHANNELS, STAGES};

/// The nominal room `room_size` spans, in metres.
const ROOM_METRES: (f32, f32) = (1.0, 100.0);
const SPEED_OF_SOUND: f32 = 343.0;
/// Added to every room's crossing time, so the smallest rooms still have
/// lines long enough to be heard as a space rather than a comb filter.
const CROSSING_FLOOR_SECONDS: f32 = 0.012;
/// The decay times `decay` spans, as RT60 in seconds.
const RT60_SECONDS: (f32, f32) = (0.2, 15.0);

/// Each feedback line's length as a fraction of the room's crossing time.
/// Spaced roughly geometrically and irregularly: the ratio of every pair
/// stays at least 0.8% away from any fraction with a denominator up to 8,
/// so no two lines' echoes soon line up again into a flutter or a pitch.
const LINE_SPREAD: [f32; CHANNELS] = [0.363, 0.409, 0.472, 0.522, 0.621, 0.674, 0.738, 0.800];
/// Each diffusion stage's longest delay as a fraction of the crossing time.
/// Each stage is a little under twice the last, so the echoes thicken
/// stage by stage without gathering at one delay.
const DIFFUSION_SPREAD: [f32; STAGES] = [0.034, 0.061, 0.109, 0.187];
/// Where in its slot of a stage's span each channel's delay falls: channel
/// `ch` reads `(ch + jitter) / CHANNELS` of the way along, so the delays
/// cover the span evenly without a regular grid.
const DIFFUSION_JITTER: [[f32; CHANNELS]; STAGES] = [
    [0.553, 0.888, 0.402, 0.613, 0.197, 0.136, 0.771, 0.262],
    [0.849, 0.896, 0.594, 0.882, 0.737, 0.819, 0.579, 0.663],
    [0.426, 0.739, 0.574, 0.741, 0.537, 0.898, 0.144, 0.169],
    [0.800, 0.159, 0.718, 0.534, 0.893, 0.116, 0.387, 0.554],
];

/// What `room_size`, `decay` and `freeze` make of the signal path: every
/// delay's read offset and every feedback line's gain.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Tuning {
    /// Read offset of each diffusion stage's delay, per channel, in samples.
    pub(super) diffusion: [[usize; CHANNELS]; STAGES],
    /// Length of each feedback line, in samples.
    pub(super) lines: [usize; CHANNELS],
    /// Gain each feedback line applies per trip round the loop.
    pub(super) gains: [f32; CHANNELS],
}

impl Tuning {
    /// The tuning for a room of `room_size` and a decay of `decay` (both
    /// 0..1), frozen or not. Runs on the audio thread when a control
    /// changes, never per sample.
    pub(super) fn new(sample_rate: u32, room_size: f32, decay: f32, freeze: bool) -> Self {
        let rate = sample_rate as f32;
        let crossing = crossing_seconds(room_size) * rate;
        let diffusion = std::array::from_fn(|stage| {
            let span = crossing * DIFFUSION_SPREAD[stage];
            std::array::from_fn(|ch| {
                let slot = (ch as f32 + DIFFUSION_JITTER[stage][ch]) / CHANNELS as f32;
                ((span * slot).round() as usize).max(1)
            })
        });
        // Odd lengths: no two lines share the factor 2.
        let lines = std::array::from_fn(|ch| (crossing * LINE_SPREAD[ch]).round() as usize | 1);
        let rt60 = RT60_SECONDS.0 + decay.clamp(0.0, 1.0) * (RT60_SECONDS.1 - RT60_SECONDS.0);
        // A trip of `len` samples loses its share of 60 dB over the RT60:
        // 10^(-3 len / (rate rt60)). Frozen, nothing is lost.
        let gains = std::array::from_fn(|ch: usize| {
            if freeze {
                1.0
            } else {
                10f32.powf(-3.0 * lines[ch] as f32 / (rate * rt60))
            }
        });
        Self {
            diffusion,
            lines,
            gains,
        }
    }

    /// The largest tuning any control can ask for: what every delay line
    /// must hold, so a change of room never allocates.
    pub(super) fn largest(sample_rate: u32) -> Self {
        Self::new(sample_rate, 1.0, 1.0, false)
    }
}

/// The time sound takes to cross a room of `room_size`, plus the floor.
fn crossing_seconds(room_size: f32) -> f32 {
    let metres = ROOM_METRES.0 + room_size.clamp(0.0, 1.0) * (ROOM_METRES.1 - ROOM_METRES.0);
    CROSSING_FLOOR_SECONDS + metres / SPEED_OF_SOUND
}

/// How long one crossfade between tunings takes. Long enough that the blend
/// of two unrelated tails is heard as a change of room rather than a click,
/// short enough that the room follows automation closely.
const FADE_SECONDS: f32 = 0.03;

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
        let frames = Self::fade_frames(sample_rate);
        Self {
            from: tuning,
            to: tuning,
            waiting: None,
            remaining: 0,
            frames,
            step: 1.0 / frames as f32,
        }
    }

    /// Samples in one fade at `sample_rate`.
    pub(super) fn fade_frames(sample_rate: u32) -> u32 {
        ((FADE_SECONDS * sample_rate as f32).round() as u32).max(1)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tuning(len: usize) -> Tuning {
        Tuning {
            lines: [len; CHANNELS],
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

    #[test]
    fn every_tuning_fits_in_the_largest() {
        for rate in [8_000, 44_100, 48_000, 192_000] {
            let largest = Tuning::largest(rate);
            for step in 0..=20 {
                let tuning = Tuning::new(rate, step as f32 / 20.0, 0.5, false);
                for ch in 0..CHANNELS {
                    assert!(tuning.lines[ch] <= largest.lines[ch]);
                    for stage in 0..STAGES {
                        let offset = tuning.diffusion[stage][ch];
                        assert!(offset >= 1 && offset <= largest.diffusion[stage][ch]);
                    }
                }
            }
        }
    }

    #[test]
    fn no_two_line_spreads_are_near_a_simple_ratio() {
        for (i, a) in LINE_SPREAD.iter().enumerate() {
            for b in &LINE_SPREAD[i + 1..] {
                let ratio = a.min(*b) / a.max(*b);
                for q in 1..=8 {
                    for p in 1..=q {
                        let fraction = p as f32 / q as f32;
                        let distance = (ratio - fraction).abs() / fraction;
                        assert!(distance > 0.008, "{a}/{b} is near {p}/{q}");
                    }
                }
            }
        }
    }

    #[test]
    fn line_lengths_are_distinct_and_odd() {
        for room in [0.0, 0.5, 1.0] {
            let lines = Tuning::new(8_000, room, 0.5, false).lines;
            for (i, a) in lines.iter().enumerate() {
                assert_eq!(a % 2, 1, "{lines:?}");
                assert!(lines[i + 1..].iter().all(|b| b != a), "{lines:?}");
            }
        }
    }

    #[test]
    fn gains_reach_minus_60_db_at_the_rt60() {
        let rate = 48_000;
        let tuning = Tuning::new(rate, 0.5, 0.5, false);
        let rt60 = 0.2 + 0.5 * 14.8;
        for ch in 0..CHANNELS {
            let trips = rt60 * rate as f32 / tuning.lines[ch] as f32;
            let db = 20.0 * trips * tuning.gains[ch].log10();
            assert!((db + 60.0).abs() < 0.01, "line {ch}: {db} dB");
        }
        let frozen = Tuning::new(rate, 0.5, 0.5, true);
        assert_eq!(frozen.gains, [1.0; CHANNELS]);
    }
}
