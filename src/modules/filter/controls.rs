//! Thread-safe controls for the Filter.

use std::sync::{Arc, Mutex};

use super::FilterType;

/// Thread-safe controls for the Filter.
///
/// All fields are wrapped in `Arc<Mutex<_>>` for real-time adjustment
/// from any thread while audio is playing.
///
/// # Example
///
/// ```rust,ignore
/// let controls: FilterControls = handles.get("filter1.controls").unwrap();
///
/// // Adjust filter in real-time
/// controls.set_cutoff(2000.0);
/// controls.set_resonance(0.7);
/// controls.set_filter_type(FilterType::HighPass);
/// ```
#[derive(Clone)]
pub struct FilterControls {
    pub(crate) cutoff: Arc<Mutex<f32>>,
    pub(crate) resonance: Arc<Mutex<f32>>,
    pub(crate) filter_type: Arc<Mutex<FilterType>>,
    pub(crate) cv_amount: Arc<Mutex<f32>>,
}

impl FilterControls {
    /// Creates new filter controls with the given initial values.
    pub fn new(cutoff: f32, resonance: f32, filter_type: FilterType, cv_amount: f32) -> Self {
        Self {
            cutoff: Arc::new(Mutex::new(cutoff.clamp(20.0, 20000.0))),
            resonance: Arc::new(Mutex::new(resonance.clamp(0.0, 1.0))),
            filter_type: Arc::new(Mutex::new(filter_type)),
            cv_amount: Arc::new(Mutex::new(cv_amount.max(0.0))),
        }
    }

    /// Gets the cutoff frequency in Hz.
    pub fn cutoff(&self) -> f32 {
        *self.cutoff.lock().unwrap()
    }

    /// Sets the cutoff frequency in Hz.
    pub fn set_cutoff(&self, value: f32) {
        *self.cutoff.lock().unwrap() = value.clamp(20.0, 20000.0);
    }

    /// Gets the resonance (0.0-1.0).
    pub fn resonance(&self) -> f32 {
        *self.resonance.lock().unwrap()
    }

    /// Sets the resonance (0.0-1.0).
    pub fn set_resonance(&self, value: f32) {
        *self.resonance.lock().unwrap() = value.clamp(0.0, 1.0);
    }

    /// Gets the filter type.
    pub fn filter_type(&self) -> FilterType {
        *self.filter_type.lock().unwrap()
    }

    /// Sets the filter type.
    pub fn set_filter_type(&self, value: FilterType) {
        *self.filter_type.lock().unwrap() = value;
    }

    /// Gets the CV modulation amount in Hz.
    pub fn cv_amount(&self) -> f32 {
        *self.cv_amount.lock().unwrap()
    }

    /// Sets the CV modulation amount in Hz.
    pub fn set_cv_amount(&self, value: f32) {
        *self.cv_amount.lock().unwrap() = value.max(0.0);
    }
}
