//! A value that glides to each new target instead of jumping.

/// A value that moves linearly to each new target over a fixed number of
/// frames. A gain that jumps between two samples steps the waveform, and the
/// step is heard as a click; the same change spread over a few milliseconds is
/// heard as the change alone.
///
/// A new target arriving mid-glide starts a fresh glide from wherever the
/// value is, so the value never jumps. The audio thread owns it: no locks, no
/// allocation.
#[derive(Clone, Copy, Debug)]
pub struct LinearRamp {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
    frames: u32,
}

impl LinearRamp {
    /// A ramp resting at `value` that takes `frames` frames to reach each new
    /// target (0 jumps at once).
    pub fn new(value: f32, frames: u32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
            frames,
        }
    }

    /// Moves to `value` at once, with no glide.
    pub fn snap(&mut self, value: f32) {
        *self = Self::new(value, self.frames);
    }

    /// Starts a glide to `target`, unless it is already the target.
    #[inline]
    pub fn set_target(&mut self, target: f32) {
        // Compared as bits, so a NaN target does not restart every block.
        if target.to_bits() == self.target.to_bits() {
            return;
        }
        self.target = target;
        // A glide from a value that is not a number would stay one until it
        // landed: start again from the target instead.
        if self.frames == 0 || !self.current.is_finite() {
            self.current = target;
            self.remaining = 0;
        } else {
            self.step = (target - self.current) / self.frames as f32;
            self.remaining = self.frames;
        }
    }

    /// The value for the next frame. The last frame of a glide is exactly the
    /// target, whatever rounding the steps gathered.
    #[inline]
    pub fn next_value(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.current = if self.remaining == 0 {
                self.target
            } else {
                self.current + self.step
            };
        }
        self.current
    }

    /// The value the last frame took (where a glide is, or the resting value).
    #[inline]
    pub fn value(&self) -> f32 {
        self.current
    }

    /// Whether the value is still moving.
    pub fn is_gliding(&self) -> bool {
        self.remaining > 0
    }
}

#[cfg(test)]
mod tests {
    use super::LinearRamp;

    const FRAMES: u32 = 4;

    #[test]
    fn rests_at_its_value_until_given_a_target() {
        let mut ramp = LinearRamp::new(0.5, FRAMES);
        assert_eq!(ramp.next_value(), 0.5);
        assert!(!ramp.is_gliding());
    }

    #[test]
    fn glides_in_equal_steps_and_lands_exactly() {
        let mut ramp = LinearRamp::new(0.0, FRAMES);
        ramp.set_target(1.0);
        let values: Vec<f32> = (0..FRAMES + 1).map(|_| ramp.next_value()).collect();
        assert_eq!(values, [0.25, 0.5, 0.75, 1.0, 1.0]);
        assert!(!ramp.is_gliding());
    }

    #[test]
    fn a_new_target_mid_glide_starts_from_where_it_is() {
        let mut ramp = LinearRamp::new(0.0, FRAMES);
        ramp.set_target(1.0);
        ramp.next_value();
        let at = ramp.next_value();
        ramp.set_target(0.0);
        let next = ramp.next_value();
        assert_eq!(at, 0.5);
        assert_eq!(next, 0.375);
    }

    #[test]
    fn the_same_target_does_not_restart_the_glide() {
        let mut ramp = LinearRamp::new(0.0, FRAMES);
        ramp.set_target(1.0);
        ramp.next_value();
        ramp.set_target(1.0);
        assert_eq!(ramp.next_value(), 0.5);
    }

    #[test]
    fn a_glide_does_not_start_from_a_value_that_is_not_a_number() {
        let mut ramp = LinearRamp::new(0.0, FRAMES);
        ramp.set_target(f32::NAN);
        ramp.snap(f32::NAN);
        ramp.set_target(1.0);
        assert_eq!(ramp.next_value(), 1.0);
    }

    #[test]
    fn snap_and_zero_frames_jump() {
        let mut ramp = LinearRamp::new(0.0, FRAMES);
        ramp.set_target(1.0);
        ramp.snap(0.25);
        assert_eq!(ramp.next_value(), 0.25);
        let mut instant = LinearRamp::new(0.0, 0);
        instant.set_target(1.0);
        assert_eq!(instant.next_value(), 1.0);
    }
}
