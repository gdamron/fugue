use crate::oscillator::OscillatorType;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MelodyParams {
    pub allowed_degrees: Arc<Mutex<Vec<usize>>>,
    pub note_weights: Arc<Mutex<Vec<f32>>>,
    pub note_duration: Arc<Mutex<f32>>,
    pub oscillator_type: Arc<Mutex<OscillatorType>>,
}

impl MelodyParams {
    pub fn new(allowed_degrees: Vec<usize>) -> Self {
        let weights = vec![1.0; allowed_degrees.len()];
        Self {
            allowed_degrees: Arc::new(Mutex::new(allowed_degrees)),
            note_weights: Arc::new(Mutex::new(weights)),
            note_duration: Arc::new(Mutex::new(1.0)), // Quarter note (1 beat)
            oscillator_type: Arc::new(Mutex::new(OscillatorType::Sine)),
        }
    }

    pub fn set_allowed_degrees(&self, degrees: Vec<usize>) {
        let mut allowed = self.allowed_degrees.lock().unwrap();
        *allowed = degrees.clone();

        let mut weights = self.note_weights.lock().unwrap();
        weights.resize(degrees.len(), 1.0);
    }

    pub fn set_note_weights(&self, weights: Vec<f32>) {
        *self.note_weights.lock().unwrap() = weights;
    }

    pub fn set_note_duration(&self, duration: f32) {
        *self.note_duration.lock().unwrap() = duration;
    }

    pub fn set_oscillator_type(&self, osc_type: OscillatorType) {
        *self.oscillator_type.lock().unwrap() = osc_type;
    }

    pub fn get_oscillator_type(&self) -> OscillatorType {
        *self.oscillator_type.lock().unwrap()
    }
}
