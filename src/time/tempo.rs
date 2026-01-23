use std::sync::{Arc, Mutex};

/// A thread-safe tempo controller measured in beats per minute (BPM).
///
/// The tempo can be adjusted in real-time from any thread while audio is playing.
#[derive(Clone)]
pub struct Tempo {
    bpm: Arc<Mutex<f64>>,
}

impl Tempo {
    /// Creates a new tempo with the given BPM value.
    pub fn new(bpm: f64) -> Self {
        Self {
            bpm: Arc::new(Mutex::new(bpm)),
        }
    }

    /// Sets the tempo to a new BPM value.
    pub fn set_bpm(&self, bpm: f64) {
        *self.bpm.lock().unwrap() = bpm;
    }

    /// Returns the current BPM value.
    pub fn get_bpm(&self) -> f64 {
        *self.bpm.lock().unwrap()
    }

    /// Calculates the number of samples per beat at the given sample rate.
    pub fn samples_per_beat(&self, sample_rate: u32) -> f64 {
        (sample_rate as f64 * 60.0) / self.get_bpm()
    }
}
