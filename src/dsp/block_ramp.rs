//! Spreading a once-per-block parameter change across the block.
//!
//! Controls arrive once per block, but a filter cutoff or a gain that
//! steps there puts a corner in the sound every block: zipper noise. A
//! [`BlockRamp`] instead moves from where the last block ended to the new
//! value in equal steps, landing on it with the block's last frame, so no
//! single frame moves more than the change divided by the block length.
//!
//! Unlike [`LinearRamp`](super::LinearRamp), which glides over a fixed
//! time whatever the block size, this always takes exactly one block: it
//! follows the control as closely as possible without stepping.

/// A parameter that ramps to each block's value across the block.
///
/// The audio thread owns it: no locks, no allocation.
#[derive(Clone, Copy, Debug)]
pub struct BlockRamp {
    /// The value the last block ended on.
    current: f32,
}

/// The frame-by-frame values of one block's ramp.
#[derive(Clone, Copy, Debug)]
pub struct BlockSteps {
    start: f32,
    step: f32,
    target: f32,
    frame: u32,
    frames: u32,
}

impl BlockRamp {
    /// A ramp resting at `value`.
    pub fn new(value: f32) -> Self {
        Self { current: value }
    }

    /// The value the last block ended on.
    pub fn value(&self) -> f32 {
        self.current
    }

    /// Moves to `value` at once: for a reset, or the first block.
    pub fn snap(&mut self, value: f32) {
        self.current = value;
    }

    /// The values for a block of `frames` frames heading to `target`. The
    /// last is exactly `target`. A target that is not a number is ignored:
    /// the value holds.
    #[inline]
    pub fn steps(&mut self, target: f32, frames: usize) -> BlockSteps {
        let start = self.current;
        let target = if target.is_nan() { start } else { target };
        // A glide that started from a value that is not a number would
        // stay one: start again at the target.
        let start = if start.is_finite() { start } else { target };
        let frames = frames.min(u32::MAX as usize) as u32;
        if frames > 0 {
            self.current = target;
        }
        BlockSteps {
            start,
            step: (target - start) / frames.max(1) as f32,
            target,
            frame: 0,
            frames,
        }
    }

    /// Writes a block's values into `out`, one per frame.
    #[inline]
    pub fn fill(&mut self, target: f32, out: &mut [f32]) {
        let steps = self.steps(target, out.len());
        for (slot, value) in out.iter_mut().zip(steps) {
            *slot = value;
        }
    }
}

impl Iterator for BlockSteps {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        if self.frame >= self.frames {
            return None;
        }
        self.frame += 1;
        Some(if self.frame == self.frames {
            self.target
        } else {
            // From the start each time, so rounding never accumulates.
            self.start + self.step * self.frame as f32
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = (self.frames - self.frame) as usize;
        (left, Some(left))
    }
}

impl ExactSizeIterator for BlockSteps {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_spreads_evenly_over_the_block_and_lands_exactly() {
        let mut ramp = BlockRamp::new(0.0);
        let mut out = [0.0; 64];
        ramp.fill(1.0, &mut out);
        assert_eq!(out[63], 1.0);
        let mut previous = 0.0;
        for value in out {
            assert!((value - previous - 1.0 / 64.0).abs() < 1e-6);
            previous = value;
        }
        // The next block starts where this one ended and holds.
        ramp.fill(1.0, &mut out);
        assert!(out.iter().all(|&v| v == 1.0));
    }

    #[test]
    fn blocks_of_any_length_follow_on() {
        let mut ramp = BlockRamp::new(0.0);
        let mut last = 0.0;
        for (frames, target) in [(1, 5.0), (7, -3.0), (0, 100.0), (333, 2.0)] {
            let start = ramp.value();
            assert_eq!(start, last);
            let values: Vec<f32> = ramp.steps(target, frames).collect();
            assert_eq!(values.len(), frames);
            // No frame moves more than the block's change over its length.
            let limit = (target - start).abs() / frames.max(1) as f32 + 1e-5;
            for value in &values {
                assert!((value - last).abs() <= limit);
                last = *value;
            }
        }
        assert_eq!(ramp.value(), 2.0);
    }

    #[test]
    fn values_that_are_not_numbers_do_not_stick() {
        let mut ramp = BlockRamp::new(0.5);
        assert!(ramp.steps(f32::NAN, 8).all(|v| v == 0.5));
        ramp.snap(f32::NAN);
        assert!(ramp.steps(1.0, 8).all(|v| v == 1.0));
    }
}
