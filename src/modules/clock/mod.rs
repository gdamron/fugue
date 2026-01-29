use std::time::Duration;

use crate::Module;

pub use self::tempo::Tempo;

mod tempo;

/// A master clock that generates timing signals for tempo-synchronized modules.
///
/// Outputs timing information via the `gate` port for triggering downstream modules.
pub struct Clock {
    sample_rate: u32,
    tempo: Tempo,
    sample_count: u64,
    beats_per_measure: u32,
    // Timing state (previously in ClockSignal)
    beats: f64,
    phase: f32,
    measure: u64,
    beat_in_measure: u32,
    // Cached output for modular routing
    cached_gate: f32,
    gate_duration: f64,         // Gate duration as fraction of beat (0.0-1.0)
    last_processed_sample: u64, // For pull-based processing
}

impl Clock {
    /// Creates a new clock with the given sample rate and tempo.
    ///
    /// Defaults to 4 beats per measure (4/4 time).
    /// Gate duration defaults to 25% of each beat (balances envelope time with silence).
    pub fn new(sample_rate: u32, tempo: Tempo) -> Self {
        let mut clock = Self {
            sample_rate,
            tempo,
            sample_count: 0,
            beats_per_measure: 4,
            beats: 0.0,
            phase: 0.0,
            measure: 0,
            beat_in_measure: 0,
            cached_gate: 0.0,
            gate_duration: 0.25, // 25% duty cycle
            last_processed_sample: 0,
        };
        clock.update_signal();
        clock.update_cached_outputs();
        clock
    }

    /// Sets the gate duration as a fraction of the beat (0.0 to 1.0).
    /// For example, 0.5 = gate HIGH for 50% of each beat.
    pub fn with_gate_duration(mut self, duration: f64) -> Self {
        self.gate_duration = duration.clamp(0.0, 1.0);
        self.update_cached_outputs();
        self
    }

    /// Sets the time signature by specifying beats per measure.
    pub fn with_time_signature(mut self, beats_per_measure: u32) -> Self {
        self.beats_per_measure = beats_per_measure;
        self.update_signal();
        self.update_cached_outputs();
        self
    }

    fn update_signal(&mut self) {
        let beats = self.beats_elapsed();
        let samples_per_beat = self.tempo.samples_per_beat(self.sample_rate);
        let phase = (self.sample_count as f64 % samples_per_beat) / samples_per_beat;
        let measure = (beats / self.beats_per_measure as f64).floor() as u64;
        let beat_in_measure = (beats % self.beats_per_measure as f64).floor() as u32;

        self.beats = beats;
        self.phase = phase as f32;
        self.measure = measure;
        self.beat_in_measure = beat_in_measure;
    }

    fn update_cached_outputs(&mut self) {
        let samples_per_beat = self.tempo.samples_per_beat(self.sample_rate);
        let sample_in_beat = self.sample_count % (samples_per_beat as u64);
        let gate_samples = (samples_per_beat * self.gate_duration) as u64;

        // Gate: PWM signal - HIGH for gate_duration% of each beat
        self.cached_gate = if sample_in_beat < gate_samples {
            1.0
        } else {
            0.0
        };
    }

    /// Advances the clock by one sample.
    pub fn tick(&mut self) {
        self.sample_count += 1;
        self.update_signal();
        self.update_cached_outputs();
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
    fn name(&self) -> &str {
        "Clock"
    }

    fn process(&mut self) -> bool {
        self.tick();
        true
    }

    fn inputs(&self) -> &[&str] {
        &[] // Clock has no inputs, it's a source
    }

    fn outputs(&self) -> &[&str] {
        &["gate"]
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("Clock has no input ports, got: {}", port))
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "gate" => Ok(self.cached_gate),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }
}
