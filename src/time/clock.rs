use std::time::Duration;

use crate::module::{Generator, Module};
use crate::signal::ClockSignal;

use super::Tempo;

/// A master clock that generates timing signals for tempo-synchronized modules.
///
/// Outputs a [`ClockSignal`] containing beat and measure information
/// that other modules can use for sequencing and synchronization.
pub struct Clock {
    sample_rate: u32,
    tempo: Tempo,
    sample_count: u64,
    beats_per_measure: u32,
    current_signal: ClockSignal,
}

impl Clock {
    /// Creates a new clock with the given sample rate and tempo.
    ///
    /// Defaults to 4 beats per measure (4/4 time).
    pub fn new(sample_rate: u32, tempo: Tempo) -> Self {
        let mut clock = Self {
            sample_rate,
            tempo,
            sample_count: 0,
            beats_per_measure: 4,
            current_signal: ClockSignal::new(0.0, 0.0, 0, 0),
        };
        clock.update_signal();
        clock
    }

    /// Sets the time signature by specifying beats per measure.
    pub fn with_time_signature(mut self, beats_per_measure: u32) -> Self {
        self.beats_per_measure = beats_per_measure;
        self.update_signal();
        self
    }

    fn update_signal(&mut self) {
        let beats = self.beats_elapsed();
        let samples_per_beat = self.tempo.samples_per_beat(self.sample_rate);
        let phase = (self.sample_count as f64 % samples_per_beat) / samples_per_beat;
        let measure = (beats / self.beats_per_measure as f64).floor() as u64;
        let beat_in_measure = (beats % self.beats_per_measure as f64).floor() as u32;

        self.current_signal = ClockSignal::new(beats, phase as f32, measure, beat_in_measure);
    }

    /// Advances the clock by one sample.
    pub fn tick(&mut self) {
        self.sample_count += 1;
        self.update_signal();
    }

    /// Returns the total number of samples elapsed since the clock started.
    pub fn samples_elapsed(&self) -> u64 {
        self.sample_count
    }

    /// Returns the total time elapsed since the clock started.
    pub fn time_elapsed(&self) -> Duration {
        Duration::from_secs_f64(self.sample_count as f64 / self.sample_rate as f64)
    }

    /// Returns the total number of beats elapsed since the clock started.
    pub fn beats_elapsed(&self) -> f64 {
        self.sample_count as f64 / self.tempo.samples_per_beat(self.sample_rate)
    }

    /// Returns a reference to the tempo controller.
    pub fn tempo(&self) -> &Tempo {
        &self.tempo
    }

    /// Returns the sample rate this clock was configured with.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

impl Module for Clock {
    fn process(&mut self) -> bool {
        self.tick();
        true
    }

    fn name(&self) -> &str {
        "Clock"
    }
}

impl Generator<ClockSignal> for Clock {
    fn output(&mut self) -> ClockSignal {
        self.current_signal
    }
}
