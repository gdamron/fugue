//! Thread-safe controls for the Vca module.

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

/// Thread-safe controls for the Vca module.
///
/// The VCA has a single control `level` which is used as the amplitude
/// multiplier when no signal is connected to the level input port.
///
/// # Example
///
/// ```rust,ignore
/// let controls: VcaControls = handles.get("vca.controls").unwrap();
///
/// // Set default level (used when no level signal connected)
/// controls.set_cv(0.5);  // 50% amplitude
/// ```
#[derive(Clone)]
pub struct VcaControls {
    pub(crate) cv: AtomicF32,
}

impl VcaControls {
    /// Creates new VCA controls with the given initial CV value.
    ///
    /// Defaults to 1.0 (unity gain / passthrough).
    pub fn new(cv: f32) -> Self {
        Self {
            cv: AtomicF32::new(cv.clamp(0.0, 1.0)),
        }
    }

    /// Gets the CV value.
    pub fn cv(&self) -> f32 {
        self.cv.load()
    }

    /// Sets the CV value (0.0-1.0).
    pub fn set_cv(&self, value: f32) {
        self.cv.store(value.clamp(0.0, 1.0));
    }
}

impl Default for VcaControls {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl ControlSurface for VcaControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("level", "Default level (when no signal connected)")
                .with_range(0.0, 1.0)
                .with_default(self.cv()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "level" => Ok(self.cv().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "level" => {
                self.set_cv(value.as_number()?);
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
    }
}
