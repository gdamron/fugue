//! Thread-safe controls for the MelodyGenerator module.

use crate::modules::oscillator::OscillatorType;
use std::sync::{Arc, Mutex};

/// Thread-safe controls for the MelodyGenerator module.
///
/// All fields are wrapped in `Arc<Mutex<_>>` for real-time adjustment
/// from any thread while audio is playing.
///
/// Note: Due to the complex types (Vec), this module exposes typed methods
/// rather than the uniform f32 get/set_control API for most parameters.
///
/// # Example
///
/// ```rust,ignore
/// let controls: MelodyControls = handles.get("melody.controls").unwrap();
///
/// // Adjust melody parameters in real-time
/// controls.set_allowed_degrees(vec![0, 2, 4, 5, 7]); // Pentatonic subset
/// controls.set_note_weights(vec![1.0, 0.5, 0.8, 0.3, 1.0]);
/// controls.set_oscillator_type(OscillatorType::Square);
/// ```
#[derive(Clone)]
pub struct MelodyControls {
    /// Scale degrees that can be selected for notes.
    pub(crate) allowed_degrees: Arc<Mutex<Vec<usize>>>,
    /// Probability weights for each allowed degree.
    pub(crate) note_weights: Arc<Mutex<Vec<f32>>>,
    /// Waveform type for the voice oscillator.
    pub(crate) oscillator_type: Arc<Mutex<OscillatorType>>,
}

impl MelodyControls {
    /// Creates new melody controls with the given allowed scale degrees.
    ///
    /// All degrees start with equal probability weight.
    /// Oscillator type defaults to sine.
    pub fn new(allowed_degrees: Vec<usize>) -> Self {
        let weights = vec![1.0; allowed_degrees.len()];
        Self {
            allowed_degrees: Arc::new(Mutex::new(allowed_degrees)),
            note_weights: Arc::new(Mutex::new(weights)),
            oscillator_type: Arc::new(Mutex::new(OscillatorType::Sine)),
        }
    }

    /// Gets the allowed scale degrees.
    pub fn allowed_degrees(&self) -> Vec<usize> {
        self.allowed_degrees.lock().unwrap().clone()
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

    /// Gets the note weights.
    pub fn note_weights(&self) -> Vec<f32> {
        self.note_weights.lock().unwrap().clone()
    }

    /// Sets the probability weights for note selection.
    ///
    /// Higher weights make that degree more likely to be chosen.
    pub fn set_note_weights(&self, weights: Vec<f32>) {
        *self.note_weights.lock().unwrap() = weights;
    }

    /// Gets the oscillator type.
    pub fn oscillator_type(&self) -> OscillatorType {
        *self.oscillator_type.lock().unwrap()
    }

    /// Sets the oscillator waveform type.
    pub fn set_oscillator_type(&self, osc_type: OscillatorType) {
        *self.oscillator_type.lock().unwrap() = osc_type;
    }

    /// Gets the oscillator type as an f32 index.
    pub fn oscillator_type_index(&self) -> f32 {
        self.oscillator_type().to_index()
    }

    /// Sets the oscillator type from an f32 index.
    pub fn set_oscillator_type_index(&self, index: f32) {
        self.set_oscillator_type(OscillatorType::from_index(index));
    }
}

// Provide a type alias for backward compatibility
/// Deprecated: Use [`MelodyControls`] instead.
#[deprecated(since = "0.2.0", note = "Renamed to MelodyControls for consistency")]
pub type MelodyParams = MelodyControls;
