/// ClockSignal - timing information at audio rate
/// Contains beat/measure data updated every sample
#[derive(Debug, Clone, Copy)]
pub struct ClockSignal {
    pub beats: f64,
    pub phase: f32, // 0.0 to 1.0 within current beat
    pub measure: u64,
    pub beat_in_measure: u32,
}

impl ClockSignal {
    pub fn new(beats: f64, phase: f32, measure: u64, beat_in_measure: u32) -> Self {
        Self {
            beats,
            phase: phase.clamp(0.0, 1.0),
            measure,
            beat_in_measure,
        }
    }
}
