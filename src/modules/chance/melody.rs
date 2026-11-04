//! The melody section: shaping a draw into a value, and a value into a
//! pitch in key.

use crate::music::scales::{in_key, nearest_in, scale_degrees, strongest_degrees};
use crate::music::Note;

/// Where `steps` turns from smooth to stepped, and from stepped to
/// quantized.
pub(super) const STEPPED_FROM: f32 = 1.0 / 3.0;
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

/// One melody channel's output: the value and frequency it holds, gliding
/// toward a new one when `steps` is smooth.
#[derive(Clone, Copy)]
pub(super) struct Channel {
    pub(super) value: f32,
    pub(super) frequency: f32,
    /// A shaped value waiting for its gate to rise.
    pub(super) pending: Option<f32>,
    target_value: f32,
    target_frequency: f32,
    value_step: f32,
    /// Per-sample frequency ratio: the glide is even in pitch.
    frequency_ratio: f32,
    glide_left: u32,
}

impl Channel {
    pub(super) fn new(shape: &Shape) -> Self {
        let mut channel = Self {
            value: 0.0,
            frequency: 0.0,
            pending: None,
            target_value: 0.0,
            target_frequency: 0.0,
            value_step: 0.0,
            frequency_ratio: 1.0,
            glide_left: 0,
        };
        channel.take(shape, shape.value_bias, 0);
        channel
    }

    /// Takes the value `value` (already shaped) and the pitch it plays,
    /// gliding there over `glide` samples (at once for 0).
    pub(super) fn take(&mut self, shape: &Shape, value: f32, glide: u32) {
        self.target_value = value;
        self.target_frequency = shape.frequency(value);
        if glide == 0 || self.frequency <= 0.0 {
            self.glide_left = 0;
            self.value = value;
            self.frequency = self.target_frequency;
            return;
        }
        let samples = glide as f32;
        self.value_step = (value - self.value) / samples;
        // One power per step, not per sample.
        self.frequency_ratio = (self.target_frequency / self.frequency).powf(1.0 / samples);
        self.glide_left = glide;
    }

    /// Advances a glide one sample, landing exactly on its target.
    #[inline]
    pub(super) fn tick(&mut self) {
        if self.glide_left == 0 {
            return;
        }
        self.glide_left -= 1;
        if self.glide_left == 0 {
            self.value = self.target_value;
            self.frequency = self.target_frequency;
        } else {
            self.value += self.value_step;
            self.frequency *= self.frequency_ratio;
        }
    }
}

/// How many samples a new value glides over, for a step `step_length`
/// samples long: the whole step at `steps` 0, none from [`STEPPED_FROM`].
pub(super) fn glide(steps: f32, step_length: Option<u32>) -> u32 {
    let share = ((STEPPED_FROM - steps) / STEPPED_FROM).clamp(0.0, 1.0);
    step_length.map_or(0, |length| (share * length as f32) as u32)
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

    #[test]
    fn a_smooth_channel_glides_evenly_in_pitch() {
        let mut shape = Shape::new();
        shape.steps = 0.0;
        shape.low_note = 57.0;
        shape.range = 24.0;
        let mut channel = Channel::new(&shape);
        channel.take(&shape, 0.0, 0);
        assert!((channel.frequency - 220.0).abs() < 1e-3);
        channel.take(&shape, 1.0, glide(0.0, Some(100)));
        for _ in 0..50 {
            channel.tick();
        }
        assert!(
            (channel.frequency - 440.0).abs() < 0.5,
            "{}",
            channel.frequency
        );
        assert!((channel.value - 0.5).abs() < 1e-4);
        for _ in 0..50 {
            channel.tick();
        }
        assert_eq!(channel.frequency, shape.frequency(1.0));
        assert_eq!(channel.value, 1.0);
        assert_eq!(glide(STEPPED_FROM, Some(100)), 0);
        assert_eq!(glide(0.0, None), 0);
    }
}
