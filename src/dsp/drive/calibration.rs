//! What `drive` means for each shape, and the gain that brings each back
//! to the level it was fed.

use std::f32::consts::TAU;
use std::sync::OnceLock;

use super::DriveShape;
use crate::dsp::{bit_step, quantize, Curve};

/// The level the automatic level is set for: a signal peaking at −12 dBFS
/// comes out about as loud as it went in, at any drive.
pub const REFERENCE_PEAK: f32 = 0.251_188_64;

/// Points across the drive range in each shape's table.
const SEGMENTS: usize = 64;
/// Samples of one sine period the calibration measures.
const CALIBRATION_SAMPLES: usize = 2048;
/// The most and least the automatic level applies.
const MAX_COMPENSATION: f32 = 4.0;
const MIN_COMPENSATION: f32 = 1.0 / 64.0;

/// Bits kept at drive 0 and at drive 1.
const BITS_AT_ZERO: f32 = 16.0;
const BITS_AT_FULL: f32 = 3.0;
/// At drive 1 one sample in this many is kept.
const LONGEST_HOLD: f32 = 64.0;

impl DriveShape {
    /// The gain in front of a curve at drive 1, in decibels. Folding needs
    /// less: past a few folds it turns to noise.
    fn max_gain_db(curve: Curve) -> f32 {
        match curve {
            Curve::Fold => 18.0,
            Curve::Soft | Curve::Hard | Curve::Asymmetric => 24.0,
        }
    }

    /// What `drive` sets for this shape: the gain in front of a curve, the
    /// grid step for bits, the fraction of samples kept for rate.
    fn amount(self, drive: f32) -> f32 {
        match self.curve() {
            Some(curve) => 10f32.powf(drive * Self::max_gain_db(curve) / 20.0),
            None if self == Self::Bits => {
                bit_step(BITS_AT_ZERO + (BITS_AT_FULL - BITS_AT_ZERO) * drive)
            }
            None => 1.0 / LONGEST_HOLD.powf(drive),
        }
    }

    /// The gain that returns a reference sine through this shape at
    /// `amount` to its own RMS, its offset removed as the DC blocker would.
    /// A sample-rate reducer keeps the level (on average) as it is.
    fn compensation(self, amount: f32) -> f32 {
        let shaped = |x: f32| match (self.curve(), self) {
            (Some(curve), _) => curve.apply(amount * x),
            (None, Self::Bits) => quantize(x, amount),
            (None, _) => x,
        };
        let (mut input, mut sum, mut squares) = (0.0f64, 0.0f64, 0.0f64);
        for n in 0..CALIBRATION_SAMPLES {
            let phase = TAU * (n as f32 + 0.5) / CALIBRATION_SAMPLES as f32;
            let x = REFERENCE_PEAK * phase.sin();
            let y = f64::from(shaped(x));
            input += f64::from(x) * f64::from(x);
            sum += y;
            squares += y * y;
        }
        let n = CALIBRATION_SAMPLES as f64;
        let mean = sum / n;
        let output = (squares / n - mean * mean).max(0.0);
        if output <= 0.0 {
            return MAX_COMPENSATION;
        }
        ((input / n / output).sqrt() as f32).clamp(MIN_COMPENSATION, MAX_COMPENSATION)
    }
}

/// One point of a shape's table.
#[derive(Clone, Copy, Debug, Default)]
struct Point {
    amount: f32,
    compensation: f32,
}

/// What drive sets for every shape, on a grid across its range: computed
/// once per process, then read without locking.
pub(super) struct Calibration {
    points: [[Point; SEGMENTS + 1]; DriveShape::COUNT],
}

impl Calibration {
    /// The table, built on first use (on a control thread, as a processor
    /// is made).
    pub(super) fn get() -> &'static Self {
        static TABLE: OnceLock<Calibration> = OnceLock::new();
        TABLE.get_or_init(|| {
            let mut points = [[Point::default(); SEGMENTS + 1]; DriveShape::COUNT];
            for (shape, row) in DriveShape::ALL.iter().zip(&mut points) {
                for (k, point) in row.iter_mut().enumerate() {
                    let amount = shape.amount(k as f32 / SEGMENTS as f32);
                    let compensation = shape.compensation(amount);
                    *point = Point {
                        amount,
                        compensation,
                    };
                }
                // Drive 0 is exactly unity, so a processor at rest in a
                // feedback loop adds no gain of its own.
                let rest = row[0].compensation;
                for point in row.iter_mut() {
                    point.compensation /= rest;
                }
            }
            Calibration { points }
        })
    }

    /// The amount and level compensation for `shape` at `drive` (0 to 1),
    /// interpolated between grid points.
    #[inline]
    pub(super) fn at(&self, shape: DriveShape, drive: f32) -> (f32, f32) {
        let position = drive.clamp(0.0, 1.0) * SEGMENTS as f32;
        let k = (position as usize).min(SEGMENTS - 1);
        let frac = position - k as f32;
        let row = &self.points[shape.index()];
        let (a, b) = (row[k], row[k + 1]);
        (
            a.amount + (b.amount - a.amount) * frac,
            a.compensation + (b.compensation - a.compensation) * frac,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_zero_is_clean_and_drive_one_reaches_each_extreme() {
        let table = Calibration::get();
        for shape in &DriveShape::ALL[..4] {
            assert_eq!(table.at(*shape, 0.0), (1.0, 1.0), "{shape:?}");
        }
        let (gain, compensation) = table.at(DriveShape::Hard, 1.0);
        assert!((gain - 15.85).abs() < 0.01);
        // A hard-clipped sine near a square: about −10 dB to match.
        assert!(compensation < 0.4, "{compensation}");
        assert_eq!(table.at(DriveShape::Bits, 0.0).0, bit_step(16.0));
        assert_eq!(table.at(DriveShape::Bits, 1.0).0, 0.25);
        assert_eq!(table.at(DriveShape::Rate, 0.0), (1.0, 1.0));
        assert!((table.at(DriveShape::Rate, 1.0).0 - 1.0 / 64.0).abs() < 1e-6);
    }
}
