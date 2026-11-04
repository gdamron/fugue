//! A lookahead brickwall limiter's gain computer and delay.

use super::LinearRamp;

/// Below this distance from where it is heading, the recovering gain lands
/// there exactly, so a signal back under the ceiling passes bit for bit.
const RECOVERY_SNAP: f64 = 1e-6;
/// The least gain the limiter computes (−180 dB). The average over the
/// lookahead keeps its precision across 1 to this; a frame louder than
/// the ceiling by more is clipped by the last clamp instead.
const LEAST_GAIN: f32 = 1e-9;

/// A stereo-linked lookahead brickwall limiter.
///
/// Each frame goes in, and the frame from `lookahead` frames earlier comes
/// out times one gain shared by both sides. The gain each frame needs (the
/// ceiling over its peak) is held as the least over the next `lookahead`
/// frames, recovers through a one-pole `release`, and is averaged over the
/// lookahead, so it comes down smoothly ahead of a peak and reaches the
/// needed gain by the time the peak is heard. A last clamp at the ceiling
/// catches rounding, so no output frame exceeds it.
///
/// The ceiling glides and travels with the audio: each frame is limited to
/// the ceiling it entered with. Everything is allocated when it is built;
/// processing allocates nothing.
pub struct LookaheadLimiter {
    lookahead: usize,
    /// The delayed audio and the ceiling each frame entered with, `lookahead`
    /// long, read and overwritten at `write`.
    left: Box<[f32]>,
    right: Box<[f32]>,
    ceilings: Box<[f32]>,
    write: usize,
    /// The least needed gain over the last `lookahead + 1` frames: a queue
    /// of (gain, frame) rising from front to back.
    hold: MinWindow,
    frame: u64,
    /// The held gain's recovery, in `f64` so its smallest steps near 1 are
    /// not lost to rounding.
    recovering: f64,
    release_coefficient: f64,
    /// The last `lookahead + 1` recovered gains and their sum, and how many
    /// of them are below 1 (none: the average is exactly 1).
    smoothing: Box<[f32]>,
    smoothing_at: usize,
    sum: f64,
    reducing: usize,
    ceiling: LinearRamp,
    sample_rate: f32,
}

impl LookaheadLimiter {
    /// A limiter delaying by `lookahead` frames (at least 1), its ceiling
    /// `ceiling` (linear) gliding over `glide_frames`, recovering with
    /// `release` seconds.
    pub fn new(
        sample_rate: u32,
        lookahead: usize,
        ceiling: f32,
        glide_frames: u32,
        release: f32,
    ) -> Self {
        let lookahead = lookahead.max(1);
        let window = lookahead + 1;
        let mut limiter = Self {
            lookahead,
            left: vec![0.0; lookahead].into_boxed_slice(),
            right: vec![0.0; lookahead].into_boxed_slice(),
            ceilings: vec![ceiling; lookahead].into_boxed_slice(),
            write: 0,
            hold: MinWindow::new(window),
            frame: 0,
            recovering: 1.0,
            release_coefficient: 1.0,
            smoothing: vec![1.0; window].into_boxed_slice(),
            smoothing_at: 0,
            sum: window as f64,
            reducing: 0,
            ceiling: LinearRamp::new(ceiling, glide_frames),
            sample_rate: sample_rate as f32,
        };
        limiter.set_release(release);
        limiter
    }

    /// How many frames the output trails the input.
    pub fn latency(&self) -> usize {
        self.lookahead
    }

    /// Glides the ceiling (linear) to `ceiling`.
    pub fn set_ceiling(&mut self, ceiling: f32) {
        self.ceiling.set_target(ceiling);
    }

    /// Ends a ceiling glide at its target, as if the limiter had always
    /// held it. For a limiter that has not run yet.
    pub fn settle(&mut self) {
        self.ceiling.settle();
        self.ceilings.fill(self.ceiling.value());
    }

    /// Sets the recovery time constant in seconds: after a peak, the gain
    /// covers 63% of its way back in `release` seconds.
    pub fn set_release(&mut self, release: f32) {
        let frames = f64::from(release * self.sample_rate).max(1.0);
        self.release_coefficient = 1.0 - (-1.0 / frames).exp();
    }

    /// Takes one frame in and returns the frame `lookahead` frames earlier,
    /// limited, and the gain applied to it.
    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32, f32) {
        let left = if left.is_finite() { left } else { 0.0 };
        let right = if right.is_finite() { right } else { 0.0 };
        let ceiling = self.ceiling.next_value();
        let peak = left.abs().max(right.abs());
        let needed = if peak > ceiling {
            (ceiling / peak).max(LEAST_GAIN)
        } else {
            1.0
        };

        let held = self.hold.push(needed, self.frame);
        self.frame += 1;
        let held = f64::from(held);
        self.recovering = if held <= self.recovering {
            held
        } else {
            let next = self.recovering + (held - self.recovering) * self.release_coefficient;
            if held - next < RECOVERY_SNAP {
                held
            } else {
                next
            }
        };
        let gain = self.smooth(self.recovering as f32);

        let at = self.write;
        let (out_left, out_right, out_ceiling) = (self.left[at], self.right[at], self.ceilings[at]);
        self.left[at] = left;
        self.right[at] = right;
        self.ceilings[at] = ceiling;
        self.write = if at + 1 == self.lookahead { 0 } else { at + 1 };
        let limit = |x: f32| (x * gain).clamp(-out_ceiling, out_ceiling);
        (limit(out_left), limit(out_right), gain)
    }

    /// Averages `gain` with the lookahead's worth of gains before it.
    #[inline]
    fn smooth(&mut self, gain: f32) -> f32 {
        let at = self.smoothing_at;
        let old = std::mem::replace(&mut self.smoothing[at], gain);
        self.sum += f64::from(gain) - f64::from(old);
        self.smoothing_at = if at + 1 == self.smoothing.len() {
            // Once per lookahead, sum afresh: rounding never builds up,
            // even under absurdly loud input, where the gains are tiny.
            self.sum = self.smoothing.iter().copied().map(f64::from).sum();
            0
        } else {
            at + 1
        };
        self.reducing = self.reducing + usize::from(gain < 1.0) - usize::from(old < 1.0);
        if self.reducing == 0 {
            // Every gain is 1: drop the rounding the sum gathered.
            self.sum = self.smoothing.len() as f64;
            1.0
        } else {
            (self.sum / self.smoothing.len() as f64).min(1.0) as f32
        }
    }
}

/// The least of the values pushed over the last `len` frames, in O(1)
/// amortised per push: a ring of (value, frame) pairs rising front to back.
struct MinWindow {
    values: Box<[f32]>,
    frames: Box<[u64]>,
    front: usize,
    count: usize,
}

impl MinWindow {
    fn new(len: usize) -> Self {
        Self {
            values: vec![0.0; len].into_boxed_slice(),
            frames: vec![0; len].into_boxed_slice(),
            front: 0,
            count: 0,
        }
    }

    /// Pushes `value` at `frame` and returns the least value of the last
    /// `len` frames, this one included.
    #[inline]
    fn push(&mut self, value: f32, frame: u64) -> f32 {
        let len = self.values.len();
        // Wraps an index less than twice the length.
        let slot = |i: usize| if i >= len { i - len } else { i };
        while self.count > 0 && self.values[slot(self.front + self.count - 1)] >= value {
            self.count -= 1;
        }
        if self.count > 0 && self.frames[self.front] + len as u64 <= frame {
            self.front = slot(self.front + 1);
            self.count -= 1;
        }
        let back = slot(self.front + self.count);
        self.values[back] = value;
        self.frames[back] = frame;
        self.count += 1;
        self.values[self.front]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn limiter(lookahead: usize) -> LookaheadLimiter {
        LookaheadLimiter::new(SR, lookahead, 0.5, 0, 0.05)
    }

    #[test]
    fn the_window_holds_the_least_of_the_last_frames() {
        let mut window = MinWindow::new(3);
        let held: Vec<f32> = [5.0, 3.0, 4.0, 6.0, 7.0, 8.0, 1.0]
            .iter()
            .enumerate()
            .map(|(frame, &v)| window.push(v, frame as u64))
            .collect();
        assert_eq!(held, [5.0, 3.0, 3.0, 3.0, 4.0, 6.0, 1.0]);
    }

    #[test]
    fn a_quiet_signal_comes_out_delayed_bit_for_bit() {
        let mut limiter = limiter(4);
        let input: Vec<f32> = (0..64).map(|n| 0.4 * (n as f32 * 0.3).sin()).collect();
        let output: Vec<f32> = input.iter().map(|&x| limiter.process(x, -x).0).collect();
        assert_eq!(&output[..4], &[0.0; 4]);
        assert_eq!(&output[4..], &input[..60]);
    }

    #[test]
    fn a_peak_is_met_at_the_ceiling_with_the_gain_already_down() {
        let mut limiter = limiter(8);
        let mut out = Vec::new();
        for n in 0..64 {
            let x = if n == 20 { 2.0 } else { 0.1 };
            out.push(limiter.process(x, 0.0));
        }
        assert!(out.iter().all(|(l, _, _)| l.abs() <= 0.5));
        assert!(
            (out[28].0 - 0.5).abs() < 1e-6,
            "peak at the ceiling: {}",
            out[28].0
        );
        // The gain falls in even steps over the lookahead before the peak.
        assert!(out[20].2 < 1.0 && out[19].2 == 1.0);
        assert!((out[28].2 - 0.25).abs() < 1e-6);
        let steps: Vec<f32> = out[19..=28].windows(2).map(|w| w[0].2 - w[1].2).collect();
        assert!(
            steps.iter().all(|s| (s - 0.75 / 9.0).abs() < 1e-6),
            "even steps: {steps:?}"
        );
    }

    #[test]
    fn after_a_peak_the_gain_recovers_to_exactly_one() {
        let mut limiter = limiter(8);
        limiter.process(2.0, 0.0);
        let gains: Vec<f32> = (0..SR).map(|_| limiter.process(0.1, 0.0).2).collect();
        assert_eq!(*gains.last().unwrap(), 1.0);
        assert!(gains.windows(2).skip(20).all(|w| w[1] >= w[0]));
    }

    /// Deterministic noise in [-1, 1).
    fn noise(state: &mut u32) -> f32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        (*state >> 8) as f32 / (1 << 23) as f32 - 1.0
    }

    #[test]
    fn no_frame_exceeds_its_ceiling_and_the_gain_alone_keeps_it_there() {
        for lookahead in [1, 24, 240] {
            let mut limiter = LookaheadLimiter::new(SR, lookahead, 0.5, 480, 0.05);
            let mut state = 0x9e37_79b9;
            let mut entered = std::collections::VecDeque::new();
            for n in 0..48_000 {
                if n % 128 == 0 {
                    let r = noise(&mut state).abs();
                    limiter.set_ceiling(0.06 + 0.94 * r);
                    limiter.set_release(0.001 + 0.5 * noise(&mut state).abs());
                }
                // Up to +120 dB: the hold and the average must do the work.
                let loud = 10f32.powf(6.0 * noise(&mut state).abs());
                let (l, r) = (loud * noise(&mut state), loud * noise(&mut state));
                let (out_l, out_r, gain) = limiter.process(l, r);
                let newest = limiter.write.checked_sub(1).unwrap_or(lookahead - 1);
                entered.push_back((l, r, limiter.ceilings[newest]));
                if entered.len() <= lookahead {
                    continue;
                }
                let (in_l, in_r, ceiling) = entered.pop_front().unwrap();
                for (out, x) in [(out_l, in_l), (out_r, in_r)] {
                    assert!(out.abs() <= ceiling, "{lookahead}: {out} over {ceiling}");
                    let unclamped = (x * gain).abs();
                    assert!(
                        unclamped <= ceiling * (1.0 + 4.0 * f32::EPSILON),
                        "{lookahead}: the clamp took {unclamped} down to {ceiling}"
                    );
                }
            }
        }
    }

    #[test]
    fn absurdly_loud_input_is_held_at_the_ceiling_not_silenced() {
        for onset in 0..10 {
            let mut limiter = LookaheadLimiter::new(SR, 8, 0.5, 0, 0.1);
            for _ in 0..onset {
                limiter.process(0.1, 0.0);
            }
            let out: Vec<f32> = (0..64).map(|_| limiter.process(1e20, 0.0).0).collect();
            assert!(out[8..].iter().all(|&y| y == 0.5), "onset {onset}: {out:?}");
        }
    }

    #[test]
    fn non_finite_input_is_silence() {
        let mut limiter = limiter(2);
        for _ in 0..8 {
            let (l, r, gain) = limiter.process(f32::NAN, f32::INFINITY);
            assert_eq!((l, r, gain), (0.0, 0.0, 1.0));
        }
    }
}
