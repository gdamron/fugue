//! A two-tap pitch shifter: a short delay line read by two taps whose
//! delays sweep at the pitch ratio, each faded by a triangle so one is
//! silent as it jumps back.
//!
//! As in Mutable Instruments' `clouds/dsp/pitch_shifter.h` (MIT, Copyright
//! 2014 Emilie Gillet), with a window scaled by sample rate. At unity the
//! taps would sit half a window apart and comb, so the sweep settles, at
//! a 1% detune for up to a second, to a phase where one tap plays alone.
//! See THIRD_PARTY_NOTICES.md.

use super::Recording;

/// The window, in seconds (2048 frames at 48 kHz).
const WINDOW_SECONDS: f32 = 0.042_67;
/// How fast the sweep settles at unity, as a share of the window a frame:
/// a detune of 1% at most, for at most a quarter window's travel.
const SETTLE: f32 = 0.01;

pub struct PitchShifter {
    left: Vec<f32>,
    right: Vec<f32>,
    mask: usize,
    write: usize,
    window: f32,
    phase: f32,
}

impl PitchShifter {
    pub fn new(sample_rate: u32) -> Self {
        let window = (WINDOW_SECONDS * sample_rate.max(1) as f32).max(16.0);
        let frames = (window as usize + 4).next_power_of_two();
        Self {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            mask: frames - 1,
            write: 0,
            window,
            phase: 0.0,
        }
    }

    /// Frames the shifter delays by at unity, where one tap plays alone.
    pub fn latency(&self) -> u32 {
        (0.5 * self.window) as u32
    }

    /// Fills the line with the recording up to `newest`, as if it had
    /// been playing it, so starting afresh leaves no gap.
    pub fn prime(&mut self, buffer: &Recording, newest: f64) {
        let length = buffer.len() as isize;
        let newest = newest.round() as isize;
        for k in 0..self.left.len() {
            let at = (newest - k as isize).rem_euclid(length) as usize;
            let (l, r) = buffer.read(at, 0.0);
            let slot = self.write.wrapping_sub(1 + k) & self.mask;
            self.left[slot] = l;
            self.right[slot] = r;
        }
        self.phase = 0.0;
    }

    /// One frame through, transposed by `ratio`.
    pub fn tick(&mut self, left: f32, right: f32, ratio: f32) -> (f32, f32) {
        self.left[self.write] = left;
        self.right[self.write] = right;
        let step = (1.0 - ratio) / self.window;
        let step = if step.abs() > 1e-7 {
            step
        } else {
            // At unity, drift to the nearest of 0, 0.5 and 1, where one tap
            // has the whole gain.
            let target = (self.phase * 2.0).round() * 0.5;
            (target - self.phase).clamp(-SETTLE, SETTLE) / self.window
        };
        self.phase += step;
        self.phase -= self.phase.floor();

        let (mut out_left, mut out_right) = (0.0, 0.0);
        for offset in [0.0, 0.5] {
            let mut phase = self.phase + offset;
            phase -= phase.floor();
            let gain = 1.0 - (2.0 * phase - 1.0).abs();
            let (l, r) = self.read(phase * self.window);
            out_left += gain * l;
            out_right += gain * r;
        }
        self.write = (self.write + 1) & self.mask;
        (out_left, out_right)
    }

    /// Both channels `delay` frames back, linearly interpolated.
    fn read(&self, delay: f32) -> (f32, f32) {
        let whole = delay as usize;
        let fraction = delay - whole as f32;
        let a = self.write.wrapping_sub(whole) & self.mask;
        let b = a.wrapping_sub(1) & self.mask;
        let blend = |x: &[f32]| x[a] + (x[b] - x[a]) * fraction;
        (blend(&self.left), blend(&self.right))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// Upward zero crossings in the shifted tone's last half second. The
    /// taps' crossfades blur a few, so a pitch is counted to within 5%.
    fn rises(ratio: f32, hz: f32) -> usize {
        let mut shifter = PitchShifter::new(SR);
        let out: Vec<f32> = (0..SR as usize)
            .map(|n| {
                let x = (std::f32::consts::TAU * hz * n as f32 / SR as f32).sin();
                shifter.tick(x, x, ratio).0
            })
            .collect();
        out[SR as usize / 2..]
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count()
    }

    #[test]
    fn a_ratio_transposes_and_unity_passes_through() {
        let up = rises(2.0, 200.0);
        assert!((190..=210).contains(&up), "an octave up: {up}");
        let down = rises(0.5, 400.0);
        assert!((95..=105).contains(&down), "an octave down: {down}");
        assert_eq!(rises(1.0, 200.0), 100);
    }

    #[test]
    fn unity_settles_to_one_tap_so_nothing_combs() {
        let mut shifter = PitchShifter::new(SR);
        for _ in 0..1_000 {
            shifter.tick(0.0, 0.0, 1.3);
        }
        // Back at unity, a tone comes through at its full level.
        let mut peak = 0.0f32;
        for n in 0..SR as usize * 2 {
            let x = (std::f32::consts::TAU * 1_000.0 * n as f32 / SR as f32).sin();
            let (y, _) = shifter.tick(x, x, 1.0);
            if n > SR as usize * 3 / 2 {
                peak = peak.max(y.abs());
            }
        }
        assert!(peak > 0.98, "a peak of {peak}");
    }
}
