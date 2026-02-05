//! Thread-safe controls for the Vca module.

use std::sync::{Arc, Mutex};

/// Thread-safe controls for the Vca module.
///
/// The VCA has a single control `cv` which is used as the amplitude multiplier
/// when no CV signal is connected to the cv input port.
///
/// # Example
///
/// ```rust,ignore
/// let controls: VcaControls = handles.get("vca.controls").unwrap();
///
/// // Set default CV level (used when no cv signal connected)
/// controls.set_cv(0.5);  // 50% amplitude
/// ```
#[derive(Clone)]
pub struct VcaControls {
    pub(crate) cv: Arc<Mutex<f32>>,
}

impl VcaControls {
    /// Creates new VCA controls with the given initial CV value.
    ///
    /// Defaults to 1.0 (unity gain / passthrough).
    pub fn new(cv: f32) -> Self {
        Self {
            cv: Arc::new(Mutex::new(cv.clamp(0.0, 1.0))),
        }
    }

    /// Gets the CV value.
    pub fn cv(&self) -> f32 {
        *self.cv.lock().unwrap()
    }

    /// Sets the CV value (0.0-1.0).
    pub fn set_cv(&self, value: f32) {
        *self.cv.lock().unwrap() = value.clamp(0.0, 1.0);
    }
}

impl Default for VcaControls {
    fn default() -> Self {
        Self::new(1.0)
    }
}
