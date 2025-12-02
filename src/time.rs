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

pub struct Clock {
    sample_rate: u32,
    tempo: Tempo,
    sample_count: u64,
}

impl Clock {
    pub fn new(sample_rate: u32, tempo: Tempo) -> Self {
        Self {
            sample_rate,
            tempo,
            sample_count: 0,
        }
    }

    pub fn tick(&mut self) {
        self.sample_count += 1;
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
