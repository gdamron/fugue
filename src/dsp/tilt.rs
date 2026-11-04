//! A tilt equaliser: one knob that leans the spectrum dark or bright.
//!
//! The signal is split by a one-pole filter at a pivot frequency; the band
//! below is turned up as the band above is turned down, or the reverse.
//! Before a distortion it chooses what distorts (bright drives the treble
//! harder, dark makes a fuzzy bass); after one it tames or sharpens the
//! harmonics.

use super::{LinearRamp, OnePole};

/// Where the two bands meet, in hertz.
pub const TILT_PIVOT_HZ: f32 = 800.0;
/// How far each band moves at full tilt, in decibels.
pub const TILT_RANGE_DB: f32 = 6.0;

/// A tilt equaliser whose setting glides, so moving it never clicks. At
/// tilt 0 it passes the signal unchanged (the two bands sum back to the
/// input).
#[derive(Clone, Copy, Debug)]
pub struct Tilt {
    split: OnePole,
    low: LinearRamp,
    high: LinearRamp,
    tilt: f32,
}

impl Tilt {
    /// A flat tilt at `sample_rate` whose changes glide over `glide_frames`.
    pub fn new(sample_rate: u32, glide_frames: u32) -> Self {
        Self {
            split: OnePole::new(sample_rate, TILT_PIVOT_HZ),
            low: LinearRamp::new(1.0, glide_frames),
            high: LinearRamp::new(1.0, glide_frames),
            tilt: 0.0,
        }
    }

    /// Leans the spectrum: −1 is darkest, 0 flat, 1 brightest. Not
    /// finite is ignored. Costs a `powf` on a change only.
    pub fn set_tilt(&mut self, tilt: f32) {
        if !tilt.is_finite() {
            return;
        }
        let tilt = tilt.clamp(-1.0, 1.0);
        if tilt.to_bits() == self.tilt.to_bits() {
            return;
        }
        self.tilt = tilt;
        let high = 10f32.powf(tilt * TILT_RANGE_DB / 20.0);
        self.high.set_target(high);
        self.low.set_target(1.0 / high);
    }

    /// The tilt set, −1 to 1.
    pub fn tilt(&self) -> f32 {
        self.tilt
    }

    /// Ends any glide at once.
    pub fn settle(&mut self) {
        self.low.settle();
        self.high.settle();
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let bands = self.split.process(x);
        let low = self.low.next_value();
        let high = self.high.next_value();
        if low == 1.0 && high == 1.0 {
            return x;
        }
        bands.lowpass * low + bands.highpass * high
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: u32 = 48_000;

    fn peak_after_settling(tilt: f32, hz: f32) -> f32 {
        let mut eq = Tilt::new(RATE, 0);
        eq.set_tilt(tilt);
        (0..RATE as usize)
            .map(|n| eq.process((TAU * hz * n as f32 / RATE as f32).sin()))
            .skip(RATE as usize / 2)
            .fold(0.0, |peak, y| peak.max(y.abs()))
    }

    #[test]
    fn flat_passes_unchanged_and_full_tilt_leans_by_the_range() {
        let mut eq = Tilt::new(RATE, 0);
        for x in [0.3, -0.7, 0.01] {
            assert_eq!(eq.process(x), x);
        }
        let db = |ratio: f32| 20.0 * ratio.log10();
        assert!((db(peak_after_settling(1.0, 12_000.0)) - 5.8).abs() < 0.4);
        assert!((db(peak_after_settling(1.0, 40.0)) + 5.9).abs() < 0.3);
        assert!((db(peak_after_settling(-1.0, 40.0)) - 5.9).abs() < 0.3);
        eq.set_tilt(f32::NAN);
        assert_eq!(eq.tilt(), 0.0);
    }
}
