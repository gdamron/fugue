//! The melody's degree table: plain data the melody owns, changed only as
//! the audio thread applies a request, so it needs no synchronization.
//!
//! What position `i` plays comes from the base scale and base weights, the
//! active count, and the values written to single positions:
//!
//! - Shrinking the count hides the positions past it; growing it again
//!   shows them as they were, written values included. A position past the
//!   count is still written and read: it plays once the count grows. The
//!   count changes no position, so moving it never overwrites one.
//! - Growing past the scale repeats it: position `i` plays `scale[i % n]`
//!   with weight `weights[i % n]`, unless a weight is given for `i`. An
//!   empty scale grows with degree 0. A position with no weight takes 1.0.
//! - A whole scale sets the count to its length and forgets the degrees
//!   written to single positions, keeping written weights; whole weights
//!   forget written weights.

use super::declared::MAX_DEGREES;

pub(super) struct Degrees {
    scale: [i32; MAX_DEGREES],
    scale_len: usize,
    given: [f32; MAX_DEGREES],
    given_len: usize,
    /// Weights written to single positions, kept over a scale replacement.
    written: [Option<f32>; MAX_DEGREES],
    count: usize,
    /// What each position plays.
    degrees: [i32; MAX_DEGREES],
    weights: [f32; MAX_DEGREES],
    /// Sum of the weights of the positions in play.
    total_weight: f32,
}

impl Degrees {
    /// A table of `scale`, with no weights given, counting its length.
    pub(super) fn new(scale: &[i32]) -> Self {
        let mut table = Self {
            scale: [0; MAX_DEGREES],
            scale_len: 0,
            given: [0.0; MAX_DEGREES],
            given_len: 0,
            written: [None; MAX_DEGREES],
            count: 0,
            degrees: [0; MAX_DEGREES],
            weights: [1.0; MAX_DEGREES],
            total_weight: 0.0,
        };
        table.set_scale(scale);
        table
    }

    /// Replaces the base scale (see the module docs).
    pub(super) fn set_scale(&mut self, scale: &[i32]) {
        let scale = &scale[..scale.len().min(MAX_DEGREES)];
        self.scale[..scale.len()].copy_from_slice(scale);
        self.scale_len = scale.len();
        self.count = scale.len();
        for i in 0..MAX_DEGREES {
            self.degrees[i] = self.base_degree(i);
            self.weights[i] = self.weight_of(i);
        }
        self.total();
    }

    /// Replaces the base weights (see the module docs).
    pub(super) fn set_weights(&mut self, given: &[f32]) {
        let given = &given[..given.len().min(MAX_DEGREES)];
        self.given[..given.len()].copy_from_slice(given);
        self.given_len = given.len();
        self.written = [None; MAX_DEGREES];
        for i in 0..MAX_DEGREES {
            self.weights[i] = self.weight_of(i);
        }
        self.total();
    }

    /// Sets the count, clamped to 1 to [`MAX_DEGREES`]; returns it.
    pub(super) fn set_count(&mut self, count: usize) -> usize {
        self.count = count.clamp(1, MAX_DEGREES);
        self.total();
        self.count
    }

    /// Writes the degree at `position` (clamped to ±127); returns it.
    pub(super) fn set_degree(&mut self, position: usize, degree: i32) -> i32 {
        self.degrees[position] = degree.clamp(-127, 127);
        self.degrees[position]
    }

    /// Writes the weight at `position` (clamped to 0 to 10); returns it.
    pub(super) fn set_weight(&mut self, position: usize, weight: f32) -> f32 {
        let weight = weight.clamp(0.0, 10.0);
        self.written[position] = Some(weight);
        self.weights[position] = weight;
        self.total();
        weight
    }

    pub(super) fn count(&self) -> usize {
        self.count
    }

    /// What `position` plays (or would, once the count reaches it).
    pub(super) fn degree(&self, position: usize) -> i32 {
        self.degrees[position]
    }

    pub(super) fn weight(&self, position: usize) -> f32 {
        self.weights[position]
    }

    /// Whether a weight was written to `position` since the last whole
    /// weights: a whole scale does not rewrite it.
    pub(super) fn weight_written(&self, position: usize) -> bool {
        self.written[position].is_some()
    }

    /// Returns `true` if no degrees are in play.
    pub(super) fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Picks a degree by weighted choice. `unit` is a uniform random value
    /// in `[0, 1)`; if the draw falls past every degree, the first degree
    /// is returned.
    ///
    /// Must not be called when [`Self::is_empty`].
    pub(super) fn choose(&self, unit: f32) -> i32 {
        let mut remaining = unit * self.total_weight;
        for i in 0..self.count {
            let weight = self.weights[i];
            if remaining < weight {
                return self.degrees[i];
            }
            remaining -= weight;
        }
        self.degrees[0]
    }

    fn base_degree(&self, i: usize) -> i32 {
        match self.scale_len {
            0 => 0,
            n => self.scale[i % n],
        }
    }

    fn weight_of(&self, i: usize) -> f32 {
        if let Some(weight) = self.written[i] {
            return weight;
        }
        match (i < self.given_len, self.scale_len) {
            (true, _) => self.given[i],
            (false, 0) => 1.0,
            (false, n) if i % n < self.given_len => self.given[i % n],
            _ => 1.0,
        }
    }

    fn total(&mut self) {
        self.total_weight = self.weights[..self.count].iter().sum();
    }
}

#[cfg(test)]
mod tests;
