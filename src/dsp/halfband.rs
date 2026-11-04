//! Halfband lowpass filters for changing the sample rate by two.
//!
//! A halfband filter cuts at a quarter of its sample rate, so every other
//! tap of its impulse response is zero. Doubling the rate (filling the gaps
//! between samples with zeros, then filtering out the images that leaves)
//! or halving it (filtering out what would alias, then dropping every other
//! sample) both need only the nonzero taps, about half the work of an
//! ordinary filter of the same length.
//!
//! The filter is a windowed sinc (the textbook window method with Kaiser's
//! window and his formulas for its length and shape), designed once when
//! the stage is built.

use std::f64::consts::PI;

/// One halving or doubling of the sample rate.
#[derive(Clone, Debug)]
pub(super) struct HalfbandStage {
    /// The nonzero off-centre taps, `h[0], h[2], …, h[L-1]` (symmetric).
    taps: Vec<f32>,
    /// How far behind the newest sample the centre tap sits, in samples at
    /// the lower rate.
    centre_delay: usize,
    /// Recent samples at the lower rate, for the off-centre taps: when
    /// doubling, the input; when halving, the first of each input pair.
    taps_history: History,
    /// When halving, the second of each input pair, for the centre tap.
    centre_history: History,
    /// When halving with an extra sample of delay, the output waiting to
    /// leave.
    pending: Option<f32>,
}

impl HalfbandStage {
    /// A stage that passes everything up to `pass` and attenuates
    /// everything from `2 - pass` up by about `attenuation_db` (the window
    /// method's target; it lands a few dB short), both as fractions of the
    /// lower rate's Nyquist frequency (so `pass` is below 1: 18 kHz at
    /// 48 kHz is 0.75).
    pub(super) fn new(pass: f64, attenuation_db: f64) -> Self {
        let full = design(pass, attenuation_db);
        let centre = full.len() / 2;
        let taps: Vec<f32> = full.iter().step_by(2).map(|&tap| tap as f32).collect();
        let centre_delay = centre / 2;
        let len = taps.len();
        Self {
            taps,
            centre_delay,
            taps_history: History::new(len),
            centre_history: History::new(len),
            pending: None,
        }
    }

    /// The same stage, with its halving output one sample later: a 4×
    /// round trip uses it to make its total delay a whole number of frames.
    pub(super) fn delayed_by_one(mut self) -> Self {
        self.pending = Some(0.0);
        self
    }

    /// The delay of a doubling followed by a halving through this design,
    /// in samples at the higher rate: each delays by the centre tap's
    /// index, `2m + 1`, so the round trip is a whole number of samples at
    /// the lower rate. An extra sample of halving delay adds two.
    pub(super) fn round_trip_delay(&self) -> usize {
        4 * self.centre_delay + 2 + 2 * usize::from(self.pending.is_some())
    }

    /// Doubles the rate: each sample of `input` becomes two of `output`,
    /// which must be twice as long.
    pub(super) fn upsample(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(output.len(), input.len() * 2);
        for (&sample, pair) in input.iter().zip(output.chunks_exact_mut(2)) {
            self.taps_history.push(sample);
            let recent = self.taps_history.recent();
            // A zero-stuffed input loses half its level; the factor 2 puts
            // it back.
            pair[0] = 2.0 * dot(&self.taps, recent);
            pair[1] = recent[recent.len() - 1 - self.centre_delay];
        }
    }

    /// Halves the rate: each pair of samples of `input` becomes one of
    /// `output`, which must be half as long.
    pub(super) fn downsample(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len() * 2);
        for (pair, out) in input.chunks_exact(2).zip(output.iter_mut()) {
            // The output lines up with the first of each pair, so the
            // off-centre taps fall on first samples and the centre tap on
            // the second sample of the pair `m + 1` back, read before this
            // pair's second sample goes in.
            self.taps_history.push(pair[0]);
            let centre = self.centre_history.recent();
            let centre = centre[centre.len() - 1 - self.centre_delay];
            self.centre_history.push(pair[1]);
            let filtered = dot(&self.taps, self.taps_history.recent()) + 0.5 * centre;
            *out = match &mut self.pending {
                Some(pending) => std::mem::replace(pending, filtered),
                None => filtered,
            };
        }
    }

    /// Clears the stage's memory.
    pub(super) fn reset(&mut self) {
        self.taps_history.clear();
        self.centre_history.clear();
        if let Some(pending) = &mut self.pending {
            *pending = 0.0;
        }
    }
}

/// Four running sums rather than one, so the compiler can vectorize the
/// loop (it may not reorder a single float sum).
#[inline]
fn dot(taps: &[f32], samples: &[f32]) -> f32 {
    let mut sums = [0.0f32; 4];
    let tap_quads = taps.chunks_exact(4);
    let sample_quads = samples.chunks_exact(4);
    let tail: f32 = tap_quads
        .remainder()
        .iter()
        .zip(sample_quads.remainder())
        .map(|(t, s)| t * s)
        .sum();
    for (t, s) in tap_quads.zip(sample_quads) {
        for k in 0..4 {
            sums[k] += t[k] * s[k];
        }
    }
    (sums[0] + sums[1]) + (sums[2] + sums[3]) + tail
}

/// The last `len` samples, oldest first, readable as one slice: each sample
/// is written twice, `len` apart, so the window never wraps.
#[derive(Clone, Debug)]
struct History {
    samples: Vec<f32>,
    len: usize,
    next: usize,
}

impl History {
    fn new(len: usize) -> Self {
        Self {
            samples: vec![0.0; 2 * len],
            len,
            next: 0,
        }
    }

    #[inline]
    fn push(&mut self, sample: f32) {
        self.samples[self.next] = sample;
        self.samples[self.next + self.len] = sample;
        self.next += 1;
        if self.next == self.len {
            self.next = 0;
        }
    }

    /// The last `len` samples, oldest first.
    #[inline]
    fn recent(&self) -> &[f32] {
        &self.samples[self.next..self.next + self.len]
    }

    fn clear(&mut self) {
        self.samples.fill(0.0);
        self.next = 0;
    }
}

/// A halfband filter's taps (length `4m + 3`, centre tap 0.5) passing up to
/// `pass` of the lower rate's Nyquist frequency with `attenuation_db` of
/// stopband rejection.
fn design(pass: f64, attenuation_db: f64) -> Vec<f64> {
    // The transition runs from pass to 2 - pass of the lower rate's
    // Nyquist; at the higher rate that is 2·(1 - pass)·π/2 radians per
    // sample, the width Kaiser's length formula takes.
    let transition = (1.0 - pass) * PI;
    let estimate = ((attenuation_db - 7.95) / (2.285 * transition)).ceil() as usize + 1;
    // Round up to 4m + 3 taps, so the centre tap falls on an odd index and
    // the zero taps on odd offsets from it.
    let len = (estimate.max(3) + 1).div_ceil(4) * 4 - 1;
    let beta = kaiser_beta(attenuation_db);
    let centre = (len / 2) as f64;
    let norm = bessel_i0(beta);
    (0..len)
        .map(|n| {
            let offset = n as f64 - centre;
            let sinc = if offset == 0.0 {
                0.5
            } else if (n % 2) == (len / 2) % 2 {
                // Even offsets from the centre are zeros of the halfband
                // sinc; set them exactly.
                0.0
            } else {
                (PI * offset / 2.0).sin() / (PI * offset)
            };
            let ratio = offset / centre;
            let window = bessel_i0(beta * (1.0 - ratio * ratio).max(0.0).sqrt()) / norm;
            sinc * window
        })
        .collect()
}

/// Kaiser's window shape for a stopband of `attenuation_db`.
fn kaiser_beta(attenuation_db: f64) -> f64 {
    if attenuation_db > 50.0 {
        0.1102 * (attenuation_db - 8.7)
    } else if attenuation_db >= 21.0 {
        0.5842 * (attenuation_db - 21.0).powf(0.4) + 0.07886 * (attenuation_db - 21.0)
    } else {
        0.0
    }
}

/// The zeroth-order modified Bessel function of the first kind, from its
/// power series.
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..64 {
        term *= half / k as f64;
        let square = term * term;
        sum += square;
        if square < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_design_is_a_symmetric_halfband_with_unity_dc_gain() {
        let taps = design(0.75, 80.0);
        assert_eq!(taps.len() % 4, 3);
        let centre = taps.len() / 2;
        assert_eq!(taps[centre], 0.5);
        for n in 0..taps.len() {
            assert_eq!(taps[n], taps[taps.len() - 1 - n]);
            let offset = n.abs_diff(centre);
            if offset != 0 && offset % 2 == 0 {
                assert_eq!(taps[n], 0.0, "tap {n}");
            }
        }
        let dc: f64 = taps.iter().sum();
        assert!((dc - 1.0).abs() < 1e-4, "dc gain {dc}");
    }

    #[test]
    fn bessel_i0_matches_known_values() {
        assert!((bessel_i0(0.0) - 1.0).abs() < 1e-15);
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-12);
        assert!((bessel_i0(5.0) - 27.239_871_823_604_45).abs() < 1e-9);
    }
}
