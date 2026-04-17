//! Schroeder allpass filter.

use super::DelayLine;

/// Schroeder allpass filter with configurable feedback coefficient.
///
/// Used as a diffuser in reverb algorithms. Passes all frequencies at
/// equal amplitude but shifts their phase, creating temporal smearing
/// without coloring the spectrum.
pub struct Allpass {
    delay: DelayLine,
    size: usize,
    coeff: f32,
}

impl Allpass {
    /// Creates a new allpass filter with the given delay size and feedback coefficient.
    pub fn new(size: usize, coeff: f32) -> Self {
        Self {
            delay: DelayLine::new(size.max(1)),
            size: size.max(1),
            coeff,
        }
    }

    /// Processes one sample through the allpass filter.
    #[inline]
    pub fn tick(&mut self, input: f32) -> f32 {
        let delayed = self.delay.read(self.size - 1);
        let w = input - delayed * self.coeff;
        let output = delayed + w * self.coeff;
        self.delay.write_and_advance(w);
        output
    }
}
