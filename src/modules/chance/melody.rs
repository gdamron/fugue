//! The melody section: shaping a draw into a value, and a value into a
//! pitch in key.

use crate::music::scales::{in_key, nearest_in, scale_degrees, strongest_degrees};
use crate::music::Note;

/// Where `steps` turns from stepped to quantized.
pub(super) const QUANTIZED_FROM: f32 = 0.5;

/// The melody controls a value is read through.
pub(super) struct Shape {
    pub(super) spread: f32,
    pub(super) value_bias: f32,
    pub(super) steps: f32,
    pub(super) key: u8,
    pub(super) scale: usize,
    pub(super) low_note: f32,
    pub(super) range: f32,
    /// The pitch classes quantized notes may take; recomputed when `key`,
    /// `scale` or `steps` change, not per step.
    mask: u16,
}

impl Shape {
    pub(super) fn new() -> Self {
        let mut shape = Self {
            spread: 0.5,
            value_bias: 0.5,
            steps: 0.5,
            key: 0,
            scale: 0,
            low_note: 48.0,
            range: 24.0,
            mask: 0,
        };
        shape.refresh();
        shape
    }

    /// Recomputes the quantizer's pitch classes: the whole scale at
    /// `steps` 0.5, narrowing to its tonic, fifth and third at 1.
    pub(super) fn refresh(&mut self) {
        let degrees = scale_degrees(self.scale);
        let narrowing = ((self.steps - QUANTIZED_FROM) / (1.0 - QUANTIZED_FROM)).clamp(0.0, 1.0);
        let fewest = degrees.len().min(3);
        let dropped = (narrowing * (degrees.len() - fewest) as f32).round() as usize;
        let intervals = strongest_degrees(degrees, degrees.len() - dropped);
        self.mask = in_key(intervals, self.key);
    }

    /// A draw (uniform in [0, 1)) as a value: within `spread` of the whole
    /// range, leaning to `value_bias`. Spread 0 is always `value_bias`;
    /// spread 1 is uniform over 0 to 1.
    pub(super) fn value(&self, draw: f32) -> f32 {
        self.value_bias + self.spread * (draw - self.value_bias)
    }

    /// The frequency a value plays: `low_note + value * range` semitones,
    /// snapped into key and scale once `steps` quantizes.
    pub(super) fn frequency(&self, value: f32) -> f32 {
        let pitch = self.low_note + value * self.range;
        if self.steps >= QUANTIZED_FROM {
            Note::new(nearest_in(self.mask, pitch)).frequency()
        } else {
            // Once per step, not per sample.
            440.0 * ((pitch - 69.0) / 12.0).exp2()
        }
    }
}

/// One melody channel's output: the value and frequency it holds.
#[derive(Clone, Copy)]
pub(super) struct Channel {
    pub(super) value: f32,
    pub(super) frequency: f32,
}

impl Channel {
    pub(super) fn new(shape: &Shape) -> Self {
        let mut channel = Self {
            value: 0.0,
            frequency: 0.0,
        };
        channel.take(shape, shape.value_bias);
        channel
    }

    /// Takes the value `value` (already shaped), and the pitch it plays.
    pub(super) fn take(&mut self, shape: &Shape, value: f32) {
        self.value = value;
        self.frequency = shape.frequency(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_widens_around_the_bias() {
        let mut shape = Shape::new();
        shape.value_bias = 0.25;
        shape.spread = 0.0;
        assert_eq!(shape.value(0.9), 0.25);
        shape.spread = 1.0;
        assert_eq!(shape.value(0.9), 0.9);
        shape.spread = 0.5;
        assert!((shape.value(0.0) - 0.125).abs() < 1e-6);
        assert!((shape.value(1.0) - 0.625).abs() < 1e-6);
    }

    #[test]
    fn quantized_pitches_narrow_toward_the_tonic_triad() {
        let mut shape = Shape::new();
        shape.key = 2;
        shape.steps = 1.0;
        shape.refresh();
        // D major's tonic triad: D, F#, A.
        assert_eq!(shape.mask, 1 << 2 | 1 << 6 | 1 << 9);
        shape.steps = 0.5;
        shape.refresh();
        assert_eq!(shape.mask.count_ones(), 7);
    }

    #[test]
    fn unquantized_pitches_are_equal_tempered() {
        let mut shape = Shape::new();
        shape.steps = 0.4;
        shape.low_note = 69.0;
        shape.range = 12.0;
        assert!((shape.frequency(0.0) - 440.0).abs() < 1e-3);
        assert!((shape.frequency(1.0) - 880.0).abs() < 1e-2);
        assert!((shape.frequency(0.5) - 622.254).abs() < 1e-2);
    }
}
