//! Waveshaping curves: the transfer functions of distortion.
//!
//! A waveshaper maps each sample through a fixed curve. A curve that bends
//! gently adds low, warm harmonics; one with corners adds a long series of
//! bright ones; one that folds back on itself turns a sine into a ringing,
//! vocal tone. Every curve here is our own math, with no tables.
//!
//! Corners alias: their harmonics run far past Nyquist and fold back as
//! inharmonic tones. The curves with corners (hard clip, fold) are run
//! through first-order antiderivative anti-aliasing: each output is the
//! curve's average over the straight line between the last two inputs,
//! `(F(x) − F(x₋₁)) / (x − x₋₁)` with `F` the curve's integral, which
//! smooths each corner over one sample. Run at an oversampled rate, the two
//! together hold aliases far below what either does alone.

/// Two inputs closer than this are treated as equal: the antiderivative
/// quotient loses precision there, and the curve at their midpoint is the
/// same value to within it.
const ADAA_EPSILON: f64 = 1.0e-6;

/// Where the soft curve reaches full scale and goes flat.
const SOFT_KNEE: f32 = 3.0;

/// A waveshaping curve. Each maps 0 to 0, has a slope of 1 there (a quiet
/// signal passes almost unchanged), and stays within ±1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curve {
    /// A tanh-like saturation: odd harmonics that fall away quickly. Warm
    /// at low drive, a smooth fuzz at high.
    Soft,
    /// Clipping at ±1: odd harmonics that fall away slowly. Crisp and
    /// aggressive.
    Hard,
    /// Folds back from ±1 like a triangle wave: bright, hollow, and
    /// moving as the drive changes.
    Fold,
    /// Soft saturation that bends harder on the negative half and flattens
    /// at −½: even harmonics too, like a valve or a diode. It leaves an
    /// offset that a DC blocker should follow.
    Asymmetric,
}

impl Curve {
    /// The curve at `x`.
    #[inline]
    pub fn apply(self, x: f32) -> f32 {
        match self {
            Self::Soft => soft(x),
            Self::Hard => x.clamp(-1.0, 1.0),
            Self::Fold => fold(x),
            Self::Asymmetric => {
                if x >= 0.0 {
                    soft(x)
                } else {
                    0.5 * soft(2.0 * x)
                }
            }
        }
    }

    /// The curve's integral at `x`, for the curves run anti-aliased.
    fn antiderivative(self, x: f64) -> Option<f64> {
        match self {
            Self::Hard if x.abs() <= 1.0 => Some(0.5 * x * x),
            Self::Hard => Some(x.abs() - 0.5),
            Self::Fold => Some(fold_antiderivative(x)),
            Self::Soft | Self::Asymmetric => None,
        }
    }
}

/// A rational fit of tanh (the [3/2] Padé approximant), exactly ±1 and flat
/// from ±3. Its slope is 1 at 0 and meets the flat part smoothly.
#[inline]
fn soft(x: f32) -> f32 {
    let x = x.clamp(-SOFT_KNEE, SOFT_KNEE);
    let x2 = x * x;
    // Clamped too: rounding near the knee can land a hair past 1.
    (x * (27.0 + x2) / (27.0 + 9.0 * x2)).clamp(-1.0, 1.0)
}

/// A triangle of period 4 through 0 at 0 and ±1 at ±1: values past ±1
/// reflect back.
#[inline]
fn fold(x: f32) -> f32 {
    let phase = (x + 1.0) * 0.25;
    let phase = phase - phase.floor();
    1.0 - (4.0 * phase - 2.0).abs()
}

/// The integral of [`fold`] from 0: a chain of parabolas, periodic, within
/// [0, ½].
fn fold_antiderivative(x: f64) -> f64 {
    let phase = (x + 1.0) * 0.25;
    // Where x sits in its period, 0 to 4, from the trough at −1.
    let s = 4.0 * (phase - phase.floor());
    let from_trough = if s < 2.0 {
        0.5 * s * s - s
    } else {
        -0.5 * s * s + 3.0 * s - 4.0
    };
    // Measured from x = 0 rather than from the trough at −1.
    from_trough + 0.5
}

/// A curve with the memory its anti-aliasing needs: run one per signal,
/// sample by sample, at the rate it should be heard at (oversampled for
/// distortion). Its output trails its input by half a sample.
#[derive(Clone, Copy, Debug)]
pub struct Waveshaper {
    curve: Curve,
    /// The last input, kept in f64: the quotient subtracts two nearby
    /// integrals, which f32 would leave noisy.
    previous: f64,
}

impl Waveshaper {
    /// A waveshaper on `curve`, at rest.
    pub fn new(curve: Curve) -> Self {
        Self {
            curve,
            previous: 0.0,
        }
    }

    /// Its curve.
    pub fn curve(&self) -> Curve {
        self.curve
    }

    /// Shapes one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let previous = std::mem::replace(&mut self.previous, x);
        let Some(integral) = self.curve.antiderivative(x) else {
            return self.curve.apply(x as f32);
        };
        let step = x - previous;
        if step.abs() < ADAA_EPSILON {
            return self.curve.apply((0.5 * (x + previous)) as f32);
        }
        let before = self.curve.antiderivative(previous).unwrap_or(0.0);
        ((integral - before) / step) as f32
    }

    /// Starts from `x` as if it had been resting there, so the next sample
    /// does not average across a jump from an old input.
    pub fn rest_at(&mut self, x: f32) {
        self.previous = f64::from(x);
    }
}

#[cfg(test)]
#[path = "waveshape_tests.rs"]
mod tests;
