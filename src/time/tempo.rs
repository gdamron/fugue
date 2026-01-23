use std::sync::{Arc, Mutex};

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
