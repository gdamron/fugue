//! Thread-safe controls for the Oscillator.

use std::sync::{Arc, Mutex};

use super::OscillatorType;

/// Thread-safe controls for the Oscillator.
///
/// All fields are wrapped in `Arc<Mutex<_>>` for real-time adjustment
/// from any thread while audio is playing.
///
/// # Example
///
/// ```rust,ignore
/// let controls: OscillatorControls = handles.get("osc1.controls").unwrap();
///
/// // Adjust oscillator in real-time
/// controls.set_frequency(880.0);
/// controls.set_oscillator_type(OscillatorType::Sawtooth);
/// controls.set_fm_amount(100.0);
/// ```
#[derive(Clone)]
pub struct OscillatorControls {
    pub(crate) frequency: Arc<Mutex<f32>>,
    pub(crate) oscillator_type: Arc<Mutex<OscillatorType>>,
    pub(crate) fm_amount: Arc<Mutex<f32>>,
    pub(crate) am_amount: Arc<Mutex<f32>>,
}

impl OscillatorControls {
    /// Creates new oscillator controls with the given initial values.
    pub fn new(
        frequency: f32,
        oscillator_type: OscillatorType,
        fm_amount: f32,
        am_amount: f32,
    ) -> Self {
        Self {
            frequency: Arc::new(Mutex::new(frequency.max(0.0))),
            oscillator_type: Arc::new(Mutex::new(oscillator_type)),
            fm_amount: Arc::new(Mutex::new(fm_amount)),
            am_amount: Arc::new(Mutex::new(am_amount.clamp(0.0, 1.0))),
        }
    }

    /// Gets the frequency in Hz.
    pub fn frequency(&self) -> f32 {
        *self.frequency.lock().unwrap()
    }

    /// Sets the frequency in Hz.
    pub fn set_frequency(&self, value: f32) {
        *self.frequency.lock().unwrap() = value.max(0.0);
    }

    /// Gets the oscillator type.
    pub fn oscillator_type(&self) -> OscillatorType {
        *self.oscillator_type.lock().unwrap()
    }

    /// Sets the oscillator type.
    pub fn set_oscillator_type(&self, value: OscillatorType) {
        *self.oscillator_type.lock().unwrap() = value;
    }

    /// Gets the FM amount in Hz.
    pub fn fm_amount(&self) -> f32 {
        *self.fm_amount.lock().unwrap()
    }

    /// Sets the FM amount in Hz.
    pub fn set_fm_amount(&self, value: f32) {
        *self.fm_amount.lock().unwrap() = value;
    }

    /// Gets the AM amount (0.0-1.0).
    pub fn am_amount(&self) -> f32 {
        *self.am_amount.lock().unwrap()
    }

    /// Sets the AM amount (0.0-1.0).
    pub fn set_am_amount(&self, value: f32) {
        *self.am_amount.lock().unwrap() = value.clamp(0.0, 1.0);
    }
}
