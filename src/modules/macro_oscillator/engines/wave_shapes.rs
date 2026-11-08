//! The waves of the wavetable banks, described as harmonic spectra.
//!
//! Every wave is Fugue's own, generated here from a formula: none is copied
//! or converted from another project's wavetables. Each bank is a grid of
//! 8 by 8 waves; a wave's column (`timbre`) and row (`morph`) are the two
//! parameters of its family, and the brightest column is always the last.
//!
//! - Bank 0, harmonic spectra: columns tilt the spectrum from dark to
//!   bright; rows choose which harmonics sound (all, odd, octaves, drawbar
//!   organ, primes and more).
//! - Bank 1, formants: columns move a resonance up the harmonic series;
//!   rows set its width and add a second and third resonance.
//! - Bank 2, sync and pulse width: columns raise a hard-synced slave
//!   oscillator's ratio from 1 to 5.5; rows morph the slave from saw through
//!   triangle and square to a narrow pulse.
//! - Bank 3, bit-reduced shapes: columns lower the sample rate and the bit
//!   depth of a shape, from mild to one bit at 8 steps a cycle; rows choose
//!   the shape.
//!
//! The time-domain families are piecewise linear, so their Fourier series
//! are computed exactly, segment by segment, with no sampling and no
//! aliasing in the tables themselves.
//!
//! Every wave is shifted in time so its fundamental is a sine starting at
//! zero: crossfading two neighbouring waves then never cancels the
//! fundamental, so a scan keeps its loudness.

use std::f64::consts::TAU;

/// Waves along each side of a bank.
pub(super) const SIDE: usize = 8;
/// How many banks there are.
pub(super) const BANKS: usize = 4;
/// Waves in every bank together.
pub(super) const WAVES: usize = BANKS * SIDE * SIDE;
/// The most harmonics a wave has, at its brightest mip level.
pub(super) const PARTIALS: usize = 128;

/// One complex Fourier coefficient.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Partial {
    pub re: f64,
    pub im: f64,
}

/// A wave's Fourier coefficients: entry `n` is harmonic `n` (entry 0, the
/// DC, is always 0).
pub(super) type Spectrum = [Partial; PARTIALS + 1];

/// The spectrum of the wave at `column` and `row` of `bank`.
pub(super) fn spectrum(bank: usize, row: usize, column: usize) -> Spectrum {
    let x = column as f64 / (SIDE - 1) as f64;
    let mut spectrum = match bank {
        0 => harmonic(row, x),
        1 => formant(row, x),
        2 => sync(row, x),
        _ => reduced(row, x),
    };
    spectrum[0] = Partial::default();
    align(&mut spectrum);
    spectrum
}

/// Shifts `spectrum` in time so its fundamental is `sin(2πt)` times its
/// amplitude: harmonic `n` turns by `n` times the fundamental's turn.
fn align(spectrum: &mut Spectrum) {
    let turn = spectrum[1].im.atan2(spectrum[1].re) + std::f64::consts::FRAC_PI_2;
    for (n, partial) in spectrum.iter_mut().enumerate() {
        let (sin, cos) = (-(n as f64) * turn).sin_cos();
        *partial = Partial {
            re: partial.re * cos - partial.im * sin,
            im: partial.re * sin + partial.im * cos,
        };
    }
}

/// A sum of sines with amplitude `amplitude(n)` for harmonic `n`.
fn sines(amplitude: impl Fn(f64) -> f64) -> Spectrum {
    let mut spectrum = [Partial::default(); PARTIALS + 1];
    for (n, partial) in spectrum.iter_mut().enumerate().skip(1) {
        // sin(2πnt) = (e^{i2πnt} - e^{-i2πnt}) / 2i.
        partial.im = -0.5 * amplitude(n as f64);
    }
    spectrum
}

/// Bank 0. The spectrum falls as `n^-p`, with `p` from 2.6 (dark) to 0.6
/// (bright), over the harmonics the row lets through.
fn harmonic(row: usize, x: f64) -> Spectrum {
    let p = 2.6 - 2.0 * x;
    let weight = |n: usize| -> f64 {
        match row {
            0 => 1.0,
            1 => (n % 2) as f64,
            2 => [0.35, 1.0][n % 2],
            3 => f64::from(n.is_power_of_two()),
            4 => f64::from([1, 2, 3, 4, 6, 8, 10, 12, 16].contains(&n)),
            5 => f64::from(!n.is_multiple_of(3)),
            6 => 1.0 + 4.0 * (-((n as f64 - 7.0) / 2.5).powi(2)).exp(),
            _ => f64::from(n == 1 || is_prime(n)),
        }
    };
    sines(|n| weight(n as usize) * n.powf(-p))
}

fn is_prime(n: usize) -> bool {
    n >= 2
        && (2..n)
            .take_while(|d| d * d <= n)
            .all(|d| !n.is_multiple_of(d))
}

/// Bank 1. A dark base (`n^-2`) under resonances shaped as bumps in log
/// frequency. The first sits at harmonic 2 to 40 across the columns.
fn formant(row: usize, x: f64) -> Spectrum {
    let first = 2.0 * 20.0f64.powf(x);
    // (ratio to the first resonance, gain, width in octaves)
    let resonances: &[(f64, f64, f64)] = match row {
        0 => &[(1.0, 1.0, 0.12)],
        1 => &[(1.0, 1.0, 0.4)],
        2 => &[(1.0, 1.0, 0.2), (1.5, 0.7, 0.2)],
        3 => &[(1.0, 1.0, 0.2), (2.0, 0.6, 0.2)],
        4 => &[(1.0, 1.0, 0.2), (2.5, 0.6, 0.18)],
        5 => &[(1.0, 1.0, 0.18), (3.5, 0.5, 0.15)],
        6 => &[(1.0, 1.0, 0.15), (5.0, 0.45, 0.12), (7.0, 0.3, 0.1)],
        _ => &[(1.0, 1.0, 0.1), (2.5, 0.6, 0.1), (4.0, 0.45, 0.1)],
    };
    sines(|n| {
        let lift: f64 = resonances
            .iter()
            .map(|&(ratio, gain, width)| {
                let octaves = (n / (first * ratio)).log2() / width;
                gain * (-0.5 * octaves * octaves).exp()
            })
            .sum();
        0.4 / (n * n) + 0.6 * lift
    })
}

/// One straight piece of a wave over a cycle of 0 to 1: from `v0` at `t0`
/// to `v1` at `t1`.
#[derive(Clone, Copy)]
struct Segment {
    t0: f64,
    t1: f64,
    v0: f64,
    v1: f64,
}

/// Adds the exact Fourier coefficients of `segment` to `spectrum`.
fn add_segment(spectrum: &mut Spectrum, segment: Segment) {
    let Segment { t0, t1, v0, v1 } = segment;
    if t1 <= t0 {
        return;
    }
    let slope = (v1 - v0) / (t1 - t0);
    for (n, partial) in spectrum.iter_mut().enumerate().skip(1) {
        let w = TAU * n as f64;
        // c_n = ∫ v(t) e^{-iwt} dt, whose antiderivative is
        // e^{-iwt} (i v(t) / w + slope / w²).
        let antiderivative = |t: f64, v: f64| -> (f64, f64) {
            let (c, s) = ((w * t).cos(), -(w * t).sin());
            let (a, b) = (slope / (w * w), v / w);
            (c * a - s * b, c * b + s * a)
        };
        let (re1, im1) = antiderivative(t1, v1);
        let (re0, im0) = antiderivative(t0, v0);
        partial.re += re1 - re0;
        partial.im += im1 - im0;
    }
}

/// A slave shape as corners `(phase, value)` over one of its cycles; two
/// corners at the same phase make a jump.
fn slave_corners(shape: usize) -> &'static [(f64, f64)] {
    const SAW: &[(f64, f64)] = &[(0.0, -1.0), (1.0, 1.0)];
    const TRIANGLE: &[(f64, f64)] = &[(0.0, -1.0), (0.5, 1.0), (1.0, -1.0)];
    const SQUARE: &[(f64, f64)] = &[(0.0, 1.0), (0.5, 1.0), (0.5, -1.0), (1.0, -1.0)];
    const THIRD: &[(f64, f64)] = &[(0.0, 1.0), (0.33, 1.0), (0.33, -1.0), (1.0, -1.0)];
    const FIFTH: &[(f64, f64)] = &[(0.0, 1.0), (0.2, 1.0), (0.2, -1.0), (1.0, -1.0)];
    const TENTH: &[(f64, f64)] = &[(0.0, 1.0), (0.1, 1.0), (0.1, -1.0), (1.0, -1.0)];
    [SAW, TRIANGLE, SQUARE, THIRD, FIFTH, TENTH][shape]
}

/// The spectrum of slave `shape` hard-synced at `ratio` times the cycle:
/// it restarts at every one of its own cycles and at the master's.
fn synced(shape: usize, ratio: f64) -> Spectrum {
    let corners = slave_corners(shape);
    let mut spectrum = [Partial::default(); PARTIALS + 1];
    let mut start = 0.0;
    while start < 1.0 {
        for pair in corners.windows(2) {
            let ((p0, v0), (p1, v1)) = (pair[0], pair[1]);
            let (t0, t1) = (start + p0 / ratio, start + p1 / ratio);
            if t0 >= 1.0 {
                break;
            }
            // A slave cycle cut short by the master's restart.
            let (t1, v1) = if t1 > 1.0 {
                (1.0, v0 + (v1 - v0) * (1.0 - t0) / (t1 - t0))
            } else {
                (t1, v1)
            };
            add_segment(&mut spectrum, Segment { t0, t1, v0, v1 });
        }
        start += 1.0 / ratio;
    }
    spectrum
}

/// Bank 2. The sync ratio rises from 1 to 5.5 across the columns; rows go
/// saw, two blends to triangle, triangle, square, then pulses of a third,
/// a fifth and a tenth.
fn sync(row: usize, x: f64) -> Spectrum {
    // Whole ratios past 1 would leave only the slave's own harmonics, an
    // octave or more above the played pitch: every ratio here is between.
    const RATIOS: [f64; SIDE] = [1.0, 1.5, 2.25, 2.75, 3.5, 4.25, 4.75, 5.5];
    let ratio = RATIOS[(x * (SIDE - 1) as f64).round() as usize];
    let (a, b, mix) = match row {
        0 => (0, 0, 0.0),
        1 => (0, 1, 1.0 / 3.0),
        2 => (0, 1, 2.0 / 3.0),
        r => (r - 2, r - 2, 0.0),
    };
    let (sa, sb) = (synced(a, ratio), synced(b, ratio));
    let mut spectrum = sa;
    for (out, (pa, pb)) in spectrum.iter_mut().zip(sa.iter().zip(sb.iter())) {
        *out = Partial {
            re: pa.re + (pb.re - pa.re) * mix,
            im: pa.im + (pb.im - pa.im) * mix,
        };
    }
    spectrum
}

/// The shapes bank 3 reduces, over one cycle of 0 to 1.
fn reduced_shape(row: usize, t: f64) -> f64 {
    let triangle = 1.0 - 4.0 * (t - 0.25 - (t - 0.25).floor() - 0.5).abs();
    match row {
        0 => (TAU * t).sin(),
        1 => 0.75 * ((TAU * t).sin() + 0.5 * (2.0 * TAU * t).sin()),
        2 => triangle,
        3 => (1.6 * triangle).clamp(-1.0, 1.0),
        4 => 2.0 * t - 1.0,
        5 => 1.0 - 2.0 * t * t,
        6 => (TAU * t).sin().powi(3),
        _ => {
            if t < 0.25 {
                1.0
            } else {
                -0.6
            }
        }
    }
}

/// Bank 3. Each column holds a shape at fewer steps a cycle and fewer
/// levels than the last, ending at one bit and 8 steps.
fn reduced(row: usize, x: f64) -> Spectrum {
    const STEPS: [usize; SIDE] = [128, 64, 48, 32, 24, 16, 12, 8];
    const LEVELS: [usize; SIDE] = [64, 32, 16, 12, 8, 6, 4, 2];
    let column = (x * (SIDE - 1) as f64).round() as usize;
    let (steps, levels) = (STEPS[column], LEVELS[column]);
    let mut spectrum = [Partial::default(); PARTIALS + 1];
    for step in 0..steps {
        let (t0, t1) = (step as f64 / steps as f64, (step + 1) as f64 / steps as f64);
        // Hold the shape's value at the middle of the step, rounded to
        // `levels` evenly spaced levels between -1 and 1.
        let v = reduced_shape(row, 0.5 * (t0 + t1)).clamp(-1.0, 1.0);
        let top = (levels - 1) as f64;
        let v = 2.0 * ((v + 1.0) * 0.5 * top).round() / top - 1.0;
        add_segment(
            &mut spectrum,
            Segment {
                t0,
                t1,
                v0: v,
                v1: v,
            },
        );
    }
    spectrum
}

/// The position of a wave among all [`WAVES`].
pub(super) const fn wave_index(bank: usize, row: usize, column: usize) -> usize {
    (bank * SIDE + row) * SIDE + column
}

/// The magnitude of a coefficient.
#[cfg(test)]
pub(super) fn magnitude(p: Partial) -> f64 {
    p.re.hypot(p.im)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// The wave's value at `t` from its spectrum, summed directly.
    fn evaluate(spectrum: &Spectrum, t: f64) -> f64 {
        spectrum
            .iter()
            .enumerate()
            .map(|(n, p)| {
                let a = TAU * n as f64 * t;
                2.0 * (p.re * a.cos() - p.im * a.sin())
            })
            .sum()
    }

    #[test]
    fn a_saw_segment_has_the_saw_series() {
        // A rising saw from -1 to 1 has harmonics of amplitude 2 / πn.
        let s = synced(0, 1.0);
        for n in 1..=8 {
            let expected = 2.0 / (PI * n as f64);
            assert!(
                (magnitude(s[n]) * 2.0 - expected).abs() < 1e-9,
                "harmonic {n}"
            );
        }
    }

    #[test]
    fn piecewise_series_reconstruct_their_shapes() {
        // Away from jumps, 128 harmonics of a sync saw land near the shape,
        // less its mean: two full ramps and a half ramp from -1 to 0 over
        // the last fifth of the cycle, so -0.1.
        let s = synced(0, 2.5);
        let mean = -0.1;
        for &(t, v) in &[
            (0.1, 2.0 * 0.25 - 1.0),
            (0.3, 2.0 * 0.75 - 1.0),
            (0.6, 2.0 * 0.5 - 1.0),
        ] {
            let v = v - mean;
            assert!(
                (evaluate(&s, t) - v).abs() < 0.03,
                "at {t}: {}",
                evaluate(&s, t)
            );
        }
        // A bit-reduced sine at one bit and 8 steps is a square.
        let r = reduced(0, 1.0);
        assert!((evaluate(&r, 0.25) - 1.0).abs() < 0.05);
        assert!((evaluate(&r, 0.75) + 1.0).abs() < 0.05);
    }

    #[test]
    fn every_fundamental_is_a_rising_sine() {
        for bank in 0..BANKS {
            for row in 0..SIDE {
                for column in 0..SIDE {
                    let s = spectrum(bank, row, column);
                    // c₁ = -i|c₁|: 2 Re(c₁ e^{i2πt}) = 2|c₁| sin(2πt).
                    assert!(
                        s[1].re.abs() < 1e-9 * magnitude(s[1]),
                        "{bank} {row} {column}"
                    );
                    assert!(s[1].im < 0.0, "{bank} {row} {column}");
                }
            }
        }
    }

    #[test]
    fn every_wave_is_finite_with_no_dc_and_a_fundamental() {
        for bank in 0..BANKS {
            for row in 0..SIDE {
                for column in 0..SIDE {
                    let s = spectrum(bank, row, column);
                    assert_eq!(s[0], Partial::default());
                    assert!(s.iter().all(|p| p.re.is_finite() && p.im.is_finite()));
                    let total: f64 = s.iter().map(|p| magnitude(*p).powi(2)).sum();
                    let first = magnitude(s[1]).powi(2);
                    // The fundamental is there, within 40 dB of the whole,
                    // so every wave is heard at the played pitch (hard sync
                    // at high ratios puts most power in upper harmonics).
                    assert!(
                        first > 1e-4 * total,
                        "bank {bank} row {row} column {column}: {}",
                        first / total
                    );
                }
            }
        }
    }
}
