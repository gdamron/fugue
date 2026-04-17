//! One-pole lowpass filter.

/// One-pole lowpass filter (damper).
///
/// A simple first-order IIR filter useful for high-frequency damping in
/// feedback paths, input bandwidth control, and parameter smoothing.
pub struct Damper {
    state: f32,
}

impl Damper {
    /// Creates a new damper with zero initial state.
    pub fn new() -> Self {
        Self { state: 0.0 }
    }

    /// Processes one sample.
    ///
    /// `coeff` controls the damping amount: 0.0 = no filtering (pass-through),
    /// 1.0 = full damping (output approaches DC).
    #[inline]
    pub fn tick(&mut self, input: f32, coeff: f32) -> f32 {
        self.state = input * (1.0 - coeff) + self.state * coeff;
        self.state
    }
}
