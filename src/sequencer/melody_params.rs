use crate::oscillator::OscillatorType;
use std::sync::{Arc, Mutex};

/// Thread-safe parameters for controlling melody generation.
///
/// All fields are wrapped in `Arc<Mutex<_>>` for real-time adjustment
/// from any thread while the melody is playing.
#[derive(Clone)]
pub struct MelodyParams {
    /// Scale degrees that can be selected for notes.
    pub allowed_degrees: Arc<Mutex<Vec<usize>>>,
    /// Probability weights for each allowed degree.
    pub note_weights: Arc<Mutex<Vec<f32>>>,
    /// Duration of each note in beats.
    pub note_duration: Arc<Mutex<f32>>,
    /// Waveform type for the voice oscillator.
    pub oscillator_type: Arc<Mutex<OscillatorType>>,
}

impl MelodyParams {
    /// Creates new melody parameters with the given allowed scale degrees.
    ///
    /// All degrees start with equal probability weight. Note duration
    /// defaults to 1 beat and oscillator type defaults to sine.
    pub fn new(allowed_degrees: Vec<usize>) -> Self {
        let weights = vec![1.0; allowed_degrees.len()];
        Self {
            allowed_degrees: Arc::new(Mutex::new(allowed_degrees)),
            note_weights: Arc::new(Mutex::new(weights)),
            note_duration: Arc::new(Mutex::new(1.0)),
            oscillator_type: Arc::new(Mutex::new(OscillatorType::Sine)),
        }
    }

    /// Sets which scale degrees can be used for note selection.
    ///
    /// Also resizes the weights vector to match.
    pub fn set_allowed_degrees(&self, degrees: Vec<usize>) {
        let mut allowed = self.allowed_degrees.lock().unwrap();
        *allowed = degrees.clone();

        let mut weights = self.note_weights.lock().unwrap();
        weights.resize(degrees.len(), 1.0);
    }

    /// Sets the probability weights for note selection.
    ///
    /// Higher weights make that degree more likely to be chosen.
    pub fn set_note_weights(&self, weights: Vec<f32>) {
        *self.note_weights.lock().unwrap() = weights;
    }

    /// Sets the duration of each note in beats.
    pub fn set_note_duration(&self, duration: f32) {
        *self.note_duration.lock().unwrap() = duration;
    }

    /// Sets the oscillator waveform type.
    pub fn set_oscillator_type(&self, osc_type: OscillatorType) {
        *self.oscillator_type.lock().unwrap() = osc_type;
    }

    /// Returns the current oscillator type.
    pub fn get_oscillator_type(&self) -> OscillatorType {
        *self.oscillator_type.lock().unwrap()
    }
}
