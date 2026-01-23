/// Timing information updated at audio rate.
///
/// Contains beat and measure data that modules can use for
/// tempo-synchronized behavior.
#[derive(Debug, Clone, Copy)]
pub struct ClockSignal {
    /// Total beats elapsed since the clock started.
    pub beats: f64,
    /// Position within the current beat (0.0 to 1.0).
    pub phase: f32,
    /// Current measure number (zero-indexed).
    pub measure: u64,
    /// Current beat within the measure (zero-indexed).
    pub beat_in_measure: u32,
}

impl ClockSignal {
    /// Creates a new clock signal with the given timing state.
    pub fn new(beats: f64, phase: f32, measure: u64, beat_in_measure: u32) -> Self {
        Self {
            beats,
            phase: phase.clamp(0.0, 1.0),
            measure,
            beat_in_measure,
        }
    }
}
