//! The wavetable banks as band-limited mip levels.
//!
//! Each wave of [`wave_shapes`](super::wave_shapes) is rendered at
//! [`LEVELS`] mip levels half an octave apart. Level `l` keeps the first
//! `128 · 2^(-l/2)` harmonics, so a reader can always pick a pair of levels
//! whose harmonics stay below the Nyquist frequency, and crossfade between
//! them as the pitch moves. Each level's table holds at least eight samples
//! per harmonic, which keeps a linear-interpolated read clean.
//!
//! The tables are built once, the first time a macro oscillator is built
//! ([`force`]), on the control thread, by inverse FFT of each wave's
//! spectrum. The audio thread only reads them. Nothing is stored in the
//! repository: the generator is the source. They take about 4 MB, shared
//! by every macro oscillator in the process.

use std::f64::consts::TAU;
use std::sync::LazyLock;

use crate::dsp::BlockRamp;

use super::wave_shapes::{spectrum, Spectrum, BANKS, PARTIALS, SIDE, WAVES};

/// Mip levels, half an octave apart.
pub(super) const LEVELS: usize = 15;
/// Table samples per kept harmonic, at least.
const OVERSAMPLING: usize = 8;
/// The smallest table.
const MIN_SIZE: usize = 32;
/// The RMS a wave is normalised to, unless that would push its peak past
/// [`PEAK`]. Waves sit at about the same loudness.
const RMS: f64 = 0.35;
const PEAK: f64 = 0.95;

/// Every wave at every mip level.
pub(super) struct Wavetables {
    /// Wave-major: wave `w`'s level `l` starts at `w * stride + offsets[l]`
    /// and holds `sizes[l] + 1` samples (the first repeats at the end).
    samples: Box<[f32]>,
    sizes: [usize; LEVELS],
    offsets: [usize; LEVELS],
    stride: usize,
}

/// Where one sample's read falls in a pair of mip levels: computed once a
/// sample and shared by every wave read at that phase.
#[derive(Clone, Copy, Debug)]
pub(super) struct MipRead {
    bright: (usize, f32),
    dark: (usize, f32),
    /// How much of the darker level is heard, 0 to 1.
    mix: f32,
}

static TABLES: LazyLock<Wavetables> = LazyLock::new(build);

/// The tables, built on first use.
pub(super) fn wavetables() -> &'static Wavetables {
    &TABLES
}

/// Builds the tables now, so the audio thread never does.
pub(super) fn force() {
    LazyLock::force(&TABLES);
}

/// How many harmonics level `level` keeps.
fn harmonics(level: usize) -> usize {
    (PARTIALS as f64 * (-0.5 * level as f64).exp2())
        .floor()
        .max(1.0) as usize
}

/// The fractional mip level to read at a frequency in cycles per sample.
///
/// Between levels `a` and `a + 1`, level `a`'s highest harmonic lies at
/// `2^(frac/2) / (2√2)` cycles per sample, never above Nyquist.
#[inline]
pub(super) fn level_position(frequency: f32) -> f32 {
    let frequency = frequency.max(1e-6);
    (2.0 * (256.0 * frequency).log2() + 1.0).clamp(0.0, (LEVELS - 1) as f32)
}

/// The mip position to ramp `level` to over a block in which the pitch
/// ramps from `from` to `to` (cycles per sample).
///
/// The block never reads brighter than its highest pitch allows: a rising
/// pitch moves the level darker at once, and a falling one brightens it a
/// block late. A ramp between the two ends' positions alone would read
/// too bright mid-block, since the position is the logarithm of a pitch
/// that ramps linearly.
#[inline]
pub(super) fn follow_pitch(level: &mut BlockRamp, from: f32, to: f32) -> f32 {
    let target = level_position(from.max(to));
    if target > level.value() {
        level.snap(target);
    }
    target
}

impl Wavetables {
    /// Where `phase` (0 to 1) falls in the levels around `position`.
    #[inline]
    pub fn locate(&self, phase: f32, position: f32) -> MipRead {
        let position = position.clamp(0.0, (LEVELS - 1) as f32);
        let level = (position as usize).min(LEVELS - 2);
        let place = |level: usize| {
            let size = self.sizes[level];
            let at = phase * size as f32;
            let index = (at as usize).min(size - 1);
            (self.offsets[level] + index, at - index as f32)
        };
        MipRead {
            bright: place(level),
            dark: place(level + 1),
            mix: position - level as f32,
        }
    }

    /// Wave `wave` at the place `read` names.
    #[inline]
    pub fn read(&self, wave: usize, read: &MipRead) -> f32 {
        let base = wave * self.stride;
        let lerp = |(index, fraction): (usize, f32)| {
            let a = self.samples[base + index];
            let b = self.samples[base + index + 1];
            a + (b - a) * fraction
        };
        let bright = lerp(read.bright);
        if read.mix == 0.0 {
            return bright;
        }
        bright + (lerp(read.dark) - bright) * read.mix
    }

    /// Level `level` of wave `wave`, without its repeated sample.
    #[cfg(test)]
    pub fn table(&self, wave: usize, level: usize) -> &[f32] {
        let start = wave * self.stride + self.offsets[level];
        &self.samples[start..start + self.sizes[level]]
    }
}

fn build() -> Wavetables {
    let mut sizes = [0; LEVELS];
    let mut offsets = [0; LEVELS];
    let mut stride = 0;
    for level in 0..LEVELS {
        sizes[level] = (OVERSAMPLING * harmonics(level))
            .next_power_of_two()
            .max(MIN_SIZE);
        offsets[level] = stride;
        stride += sizes[level] + 1;
    }
    let mut samples = vec![0.0f32; WAVES * stride];
    let mut fft = InverseFft::new(sizes[0]);
    for bank in 0..BANKS {
        for row in 0..SIDE {
            for column in 0..SIDE {
                let wave = super::wave_shapes::wave_index(bank, row, column);
                let spectrum = spectrum(bank, row, column);
                let base = wave * stride;
                let gain = normalising_gain(&mut fft, &spectrum, &sizes);
                for level in 0..LEVELS {
                    let table = fft.render(&spectrum, harmonics(level), sizes[level]);
                    let out = &mut samples[base + offsets[level]..][..=sizes[level]];
                    for (o, v) in out.iter_mut().zip(table.iter()) {
                        *o = (v * gain) as f32;
                    }
                    out[sizes[level]] = out[0];
                }
            }
        }
    }
    Wavetables {
        samples: samples.into_boxed_slice(),
        sizes,
        offsets,
        stride,
    }
}

/// The gain that brings the brightest level of a wave to [`RMS`], or its
/// highest peak at any level to [`PEAK`], whichever is quieter. (Dropping
/// harmonics can raise a wave's peak.)
fn normalising_gain(fft: &mut InverseFft, spectrum: &Spectrum, sizes: &[usize; LEVELS]) -> f64 {
    let mut peak = 0.0f64;
    let mut rms = 0.0;
    for (level, &size) in sizes.iter().enumerate() {
        let table = fft.render(spectrum, harmonics(level), size);
        peak = table.iter().fold(peak, |m, v| m.max(v.abs()));
        if level == 0 {
            rms = (table.iter().map(|v| v * v).sum::<f64>() / size as f64).sqrt();
        }
    }
    if peak == 0.0 {
        return 0.0;
    }
    (PEAK / peak).min(RMS / rms)
}

/// A radix-2 inverse FFT, from a wave's harmonics to one cycle of samples.
struct InverseFft {
    re: Vec<f64>,
    im: Vec<f64>,
}

impl InverseFft {
    fn new(max_size: usize) -> Self {
        Self {
            re: vec![0.0; max_size],
            im: vec![0.0; max_size],
        }
    }

    /// One cycle of `size` samples holding the first `keep` harmonics of
    /// `spectrum`: `2 Re Σ c_n e^{i2πnk/size}`.
    fn render(&mut self, spectrum: &Spectrum, keep: usize, size: usize) -> &[f64] {
        let (re, im) = (&mut self.re[..size], &mut self.im[..size]);
        re.fill(0.0);
        im.fill(0.0);
        for (n, partial) in spectrum.iter().enumerate().take(keep.min(size / 2 - 1) + 1) {
            re[n] = 2.0 * partial.re;
            im[n] = 2.0 * partial.im;
        }
        inverse_transform(re, im);
        re
    }
}

/// In-place `x[k] = Σ X[n] e^{+i2πnk/N}` for a power-of-two `N`.
fn inverse_transform(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut span = 1;
    while span < n {
        let step = TAU / (2 * span) as f64;
        for k in 0..span {
            let (s, c) = (step * k as f64).sin_cos();
            for start in (0..n).step_by(2 * span) {
                let (a, b) = (start + k, start + k + span);
                let (br, bi) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                re[b] = re[a] - br;
                im[b] = im[a] - bi;
                re[a] += br;
                im[a] += bi;
            }
        }
        span *= 2;
    }
}

#[cfg(test)]
#[path = "wavetables_tests.rs"]
mod tests;
