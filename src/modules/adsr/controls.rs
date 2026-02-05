//! Thread-safe controls for the ADSR envelope generator.

use std::sync::{Arc, Mutex};

/// Thread-safe controls for the ADSR envelope generator.
///
/// All fields are wrapped in `Arc<Mutex<_>>` for real-time adjustment
/// from any thread while audio is playing.
///
/// # Example
///
/// ```rust,ignore
/// let controls: AdsrControls = handles.get("adsr1.controls").unwrap();
///
/// // Adjust envelope shape in real-time
/// controls.set_attack(0.5);
/// controls.set_decay(0.3);
/// controls.set_sustain(0.6);
/// controls.set_release(1.0);
/// ```
#[derive(Clone)]
pub struct AdsrControls {
    pub(crate) attack: Arc<Mutex<f32>>,
    pub(crate) decay: Arc<Mutex<f32>>,
    pub(crate) sustain: Arc<Mutex<f32>>,
    pub(crate) release: Arc<Mutex<f32>>,
}

impl AdsrControls {
    /// Creates new ADSR controls with the given initial values.
    pub fn new(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        Self {
            attack: Arc::new(Mutex::new(attack.max(0.0))),
            decay: Arc::new(Mutex::new(decay.max(0.0))),
            sustain: Arc::new(Mutex::new(sustain.clamp(0.0, 1.0))),
            release: Arc::new(Mutex::new(release.max(0.0))),
        }
    }

    /// Gets the attack time in seconds.
    pub fn attack(&self) -> f32 {
        *self.attack.lock().unwrap()
    }

    /// Sets the attack time in seconds.
    pub fn set_attack(&self, value: f32) {
        *self.attack.lock().unwrap() = value.max(0.0);
    }

    /// Gets the decay time in seconds.
    pub fn decay(&self) -> f32 {
        *self.decay.lock().unwrap()
    }

    /// Sets the decay time in seconds.
    pub fn set_decay(&self, value: f32) {
        *self.decay.lock().unwrap() = value.max(0.0);
    }

    /// Gets the sustain level (0.0-1.0).
    pub fn sustain(&self) -> f32 {
        *self.sustain.lock().unwrap()
    }

    /// Sets the sustain level (0.0-1.0).
    pub fn set_sustain(&self, value: f32) {
        *self.sustain.lock().unwrap() = value.clamp(0.0, 1.0);
    }

    /// Gets the release time in seconds.
    pub fn release(&self) -> f32 {
        *self.release.lock().unwrap()
    }

    /// Sets the release time in seconds.
    pub fn set_release(&self, value: f32) {
        *self.release.lock().unwrap() = value.max(0.0);
    }
}
