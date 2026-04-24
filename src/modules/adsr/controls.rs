//! Thread-safe controls for the ADSR envelope generator.

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

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
    pub(crate) attack: AtomicF32,
    pub(crate) decay: AtomicF32,
    pub(crate) sustain: AtomicF32,
    pub(crate) release: AtomicF32,
}

impl AdsrControls {
    /// Creates new ADSR controls with the given initial values.
    pub fn new(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        Self {
            attack: AtomicF32::new(attack.max(0.0)),
            decay: AtomicF32::new(decay.max(0.0)),
            sustain: AtomicF32::new(sustain.clamp(0.0, 1.0)),
            release: AtomicF32::new(release.max(0.0)),
        }
    }

    /// Gets the attack time in seconds.
    pub fn attack(&self) -> f32 {
        self.attack.load()
    }

    /// Sets the attack time in seconds.
    pub fn set_attack(&self, value: f32) {
        self.attack.store(value.max(0.0));
    }

    /// Gets the decay time in seconds.
    pub fn decay(&self) -> f32 {
        self.decay.load()
    }

    /// Sets the decay time in seconds.
    pub fn set_decay(&self, value: f32) {
        self.decay.store(value.max(0.0));
    }

    /// Gets the sustain level (0.0-1.0).
    pub fn sustain(&self) -> f32 {
        self.sustain.load()
    }

    /// Sets the sustain level (0.0-1.0).
    pub fn set_sustain(&self, value: f32) {
        self.sustain.store(value.clamp(0.0, 1.0));
    }

    /// Gets the release time in seconds.
    pub fn release(&self) -> f32 {
        self.release.load()
    }

    /// Sets the release time in seconds.
    pub fn set_release(&self, value: f32) {
        self.release.store(value.max(0.0));
    }
}

impl ControlSurface for AdsrControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("attack", "Attack time in seconds")
                .with_range(0.0, 10.0)
                .with_default(self.attack()),
            ControlMeta::number("decay", "Decay time in seconds")
                .with_range(0.0, 10.0)
                .with_default(self.decay()),
            ControlMeta::number("sustain", "Sustain level")
                .with_range(0.0, 1.0)
                .with_default(self.sustain()),
            ControlMeta::number("release", "Release time in seconds")
                .with_range(0.0, 10.0)
                .with_default(self.release()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "attack" => Ok(self.attack().into()),
            "decay" => Ok(self.decay().into()),
            "sustain" => Ok(self.sustain().into()),
            "release" => Ok(self.release().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let value = value.as_number()?;
        match key {
            "attack" => self.set_attack(value),
            "decay" => self.set_decay(value),
            "sustain" => self.set_sustain(value),
            "release" => self.set_release(value),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
