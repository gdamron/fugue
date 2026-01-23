use crate::module::{Generator, Module};
use crate::signal::ClockSignal;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct Tempo {
    bpm: Arc<Mutex<f64>>,
}

impl Tempo {
    pub fn new(bpm: f64) -> Self {
        Self {
            bpm: Arc::new(Mutex::new(bpm)),
        }
    }

    pub fn set_bpm(&self, bpm: f64) {
        *self.bpm.lock().unwrap() = bpm;
    }

    pub fn get_bpm(&self) -> f64 {
        *self.bpm.lock().unwrap()
    }

    pub fn samples_per_beat(&self, sample_rate: u32) -> f64 {
        (sample_rate as f64 * 60.0) / self.get_bpm()
    }
}

/// Clock - a pure generator module that outputs ClockSignal
/// Does not accept input signals (like a master clock in Eurorack)
pub struct Clock {
    sample_rate: u32,
    tempo: Tempo,
    sample_count: u64,
    beats_per_measure: u32,
    current_signal: ClockSignal,
}

impl Clock {
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

    pub fn tick(&mut self) {
        self.sample_count += 1;
        self.update_signal();
    }

    pub fn samples_elapsed(&self) -> u64 {
        self.sample_count
    }

    pub fn time_elapsed(&self) -> Duration {
        Duration::from_secs_f64(self.sample_count as f64 / self.sample_rate as f64)
    }

    pub fn beats_elapsed(&self) -> f64 {
        self.sample_count as f64 / self.tempo.samples_per_beat(self.sample_rate)
    }

    pub fn tempo(&self) -> &Tempo {
        &self.tempo
    }

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
