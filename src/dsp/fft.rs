//! Radix-2 fast Fourier transform for analysis (never the audio path).
//!
//! Tables and scratch buffers are built once per size, so a transform makes no
//! allocations. Analysis runs off the audio thread; this lives beside the other
//! DSP primitives because it is a reusable building block, not a module.

use std::f32::consts::PI;

/// Magnitude spectrum of real input, using a precomputed radix-2 transform.
///
/// Real input is transformed as complex data with zero imaginary parts. That
/// costs about twice a real-optimized transform, which at analysis sizes (up
/// to 4096 points, a few hundred times a second) is far cheaper than the
/// complexity of a split-radix real transform.
pub(crate) struct RealFft {
    size: usize,
    /// Twiddles for the whole transform: `cos[j]`/`sin[j]` hold
    /// `cos(-2πj/size)` and `sin(-2πj/size)` for `j < size / 2`.
    cos: Vec<f32>,
    sin: Vec<f32>,
    /// Bit-reversal permutation of `0..size`.
    reversed: Vec<u32>,
    re: Vec<f32>,
    im: Vec<f32>,
}

impl RealFft {
    /// Builds the tables for a transform of `size` points.
    ///
    /// # Panics
    ///
    /// Panics unless `size` is a power of two of at least 2.
    pub(crate) fn new(size: usize) -> Self {
        assert!(
            size >= 2 && size.is_power_of_two(),
            "fft size must be a power of two"
        );
        let half = size / 2;
        let mut cos = Vec::with_capacity(half);
        let mut sin = Vec::with_capacity(half);
        for j in 0..half {
            let angle = -2.0 * PI * (j as f32) / (size as f32);
            cos.push(angle.cos());
            sin.push(angle.sin());
        }
        let bits = size.trailing_zeros();
        let reversed = (0..size)
            .map(|i| (i as u32).reverse_bits() >> (32 - bits))
            .collect();
        Self {
            size,
            cos,
            sin,
            reversed,
            re: vec![0.0; size],
            im: vec![0.0; size],
        }
    }

    /// Number of magnitudes produced, from DC to Nyquist inclusive.
    pub(crate) fn bin_count(&self) -> usize {
        self.size / 2 + 1
    }

    /// Writes the magnitudes of `input` into `out`, allocating nothing.
    ///
    /// # Panics
    ///
    /// Panics unless `input` holds [`size`](Self::size) samples and `out` holds
    /// [`bin_count`](Self::bin_count) values.
    pub(crate) fn magnitudes(&mut self, input: &[f32], out: &mut [f32]) {
        assert_eq!(input.len(), self.size, "input must hold size samples");
        assert_eq!(
            out.len(),
            self.bin_count(),
            "out must hold bin_count values"
        );

        // Load in bit-reversed order, so the butterflies below run in place.
        for i in 0..self.size {
            self.re[i] = input[self.reversed[i] as usize];
            self.im[i] = 0.0;
        }

        // Cooley-Tukey: combine pairs of half-size transforms, doubling the
        // span each stage until it covers the whole buffer.
        let mut span = 2;
        while span <= self.size {
            let stride = self.size / span;
            for start in (0..self.size).step_by(span) {
                for k in 0..span / 2 {
                    let twiddle = k * stride;
                    let (wr, wi) = (self.cos[twiddle], self.sin[twiddle]);
                    let even = start + k;
                    let odd = even + span / 2;
                    let tr = self.re[odd] * wr - self.im[odd] * wi;
                    let ti = self.re[odd] * wi + self.im[odd] * wr;
                    self.re[odd] = self.re[even] - tr;
                    self.im[odd] = self.im[even] - ti;
                    self.re[even] += tr;
                    self.im[even] += ti;
                }
            }
            span <<= 1;
        }

        for (bin, value) in out.iter_mut().enumerate() {
            *value = self.re[bin].hypot(self.im[bin]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Straightforward transform, slow but obviously correct.
    fn naive_magnitudes(input: &[f32]) -> Vec<f32> {
        let n = input.len();
        (0..n / 2 + 1)
            .map(|k| {
                let (mut re, mut im) = (0.0f32, 0.0f32);
                for (i, sample) in input.iter().enumerate() {
                    let angle = -2.0 * PI * (k as f32) * (i as f32) / (n as f32);
                    re += sample * angle.cos();
                    im += sample * angle.sin();
                }
                re.hypot(im)
            })
            .collect()
    }

    fn tone(size: usize, bin: f32, amplitude: f32) -> Vec<f32> {
        (0..size)
            .map(|i| amplitude * (2.0 * PI * bin * (i as f32) / (size as f32)).sin())
            .collect()
    }

    #[test]
    fn matches_a_naive_transform() {
        let mut fft = RealFft::new(64);
        let input: Vec<f32> = (0..64)
            .map(|i| (i as f32 * 0.7).sin() * 0.4 + (i as f32 * 0.13).cos() * 0.25)
            .collect();
        let mut out = vec![0.0; fft.bin_count()];
        fft.magnitudes(&input, &mut out);
        for (got, want) in out.iter().zip(naive_magnitudes(&input)) {
            assert!((got - want).abs() < 1e-3, "got {got}, want {want}");
        }
    }

    #[test]
    fn concentrates_a_bin_centred_tone_in_one_bin() {
        let size = 256;
        let mut fft = RealFft::new(size);
        let mut out = vec![0.0; fft.bin_count()];
        fft.magnitudes(&tone(size, 8.0, 0.5), &mut out);

        // A sine at a bin centre splits its energy between +f and -f, so the
        // peak is amplitude * size / 2.
        let peak = out[8];
        assert!(
            (peak - 0.5 * size as f32 / 2.0).abs() < 1e-1,
            "peak {peak} at bin 8"
        );
        for (bin, value) in out.iter().enumerate() {
            if bin != 8 {
                assert!(*value < peak * 1e-3, "bin {bin} leaked {value}");
            }
        }
    }

    #[test]
    fn reports_silence_as_zero() {
        let mut fft = RealFft::new(32);
        let mut out = vec![0.0; fft.bin_count()];
        fft.magnitudes(&[0.0; 32], &mut out);
        assert!(out.iter().all(|m| *m == 0.0));
    }

    #[test]
    fn puts_dc_and_nyquist_at_the_ends() {
        let size = 32;
        let mut fft = RealFft::new(size);
        let mut out = vec![0.0; fft.bin_count()];

        fft.magnitudes(&[1.0; 32], &mut out);
        assert!((out[0] - size as f32).abs() < 1e-3);

        let alternating: Vec<f32> = (0..size)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        fft.magnitudes(&alternating, &mut out);
        assert!((out[size / 2] - size as f32).abs() < 1e-3);
    }

    #[test]
    fn transforms_every_supported_size() {
        for bits in 2..=12 {
            let size = 1usize << bits;
            let mut fft = RealFft::new(size);
            assert_eq!(fft.bin_count(), size / 2 + 1);
            let mut out = vec![0.0; fft.bin_count()];
            fft.magnitudes(&tone(size, 1.0, 0.25), &mut out);
            assert!(out[1] > out[0] * 10.0, "size {size} missed its tone");
        }
    }
}
