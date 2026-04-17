//! Thread-safe controls for the Reverb module.

use std::sync::{Arc, Mutex};

use crate::{ControlMeta, ControlSurface, ControlValue};

/// Thread-safe controls for the Reverb module.
///
/// All fields are wrapped in `Arc<Mutex<_>>` for real-time adjustment
/// from any thread while audio is playing.
#[derive(Clone)]
pub struct ReverbControls {
    pub(crate) room_size: Arc<Mutex<f32>>,
    pub(crate) decay: Arc<Mutex<f32>>,
    pub(crate) damping: Arc<Mutex<f32>>,
    pub(crate) wet: Arc<Mutex<f32>>,
    pub(crate) dry: Arc<Mutex<f32>>,
    pub(crate) width: Arc<Mutex<f32>>,
    pub(crate) freeze: Arc<Mutex<bool>>,
}

impl ReverbControls {
    /// Creates new reverb controls with the given initial values.
    pub fn new(
        room_size: f32,
        decay: f32,
        damping: f32,
        wet: f32,
        dry: f32,
        width: f32,
        freeze: bool,
    ) -> Self {
        Self {
            room_size: Arc::new(Mutex::new(room_size.clamp(0.0, 1.0))),
            decay: Arc::new(Mutex::new(decay.clamp(0.0, 1.0))),
            damping: Arc::new(Mutex::new(damping.clamp(0.0, 1.0))),
            wet: Arc::new(Mutex::new(wet.clamp(0.0, 1.0))),
            dry: Arc::new(Mutex::new(dry.clamp(0.0, 1.0))),
            width: Arc::new(Mutex::new(width.clamp(0.0, 1.0))),
            freeze: Arc::new(Mutex::new(freeze)),
        }
    }

    pub fn room_size(&self) -> f32 {
        *self.room_size.lock().unwrap()
    }

    pub fn set_room_size(&self, value: f32) {
        *self.room_size.lock().unwrap() = value.clamp(0.0, 1.0);
    }

    pub fn decay(&self) -> f32 {
        *self.decay.lock().unwrap()
    }

    pub fn set_decay(&self, value: f32) {
        *self.decay.lock().unwrap() = value.clamp(0.0, 1.0);
    }

    pub fn damping(&self) -> f32 {
        *self.damping.lock().unwrap()
    }

    pub fn set_damping(&self, value: f32) {
        *self.damping.lock().unwrap() = value.clamp(0.0, 1.0);
    }

    pub fn wet(&self) -> f32 {
        *self.wet.lock().unwrap()
    }

    pub fn set_wet(&self, value: f32) {
        *self.wet.lock().unwrap() = value.clamp(0.0, 1.0);
    }

    pub fn dry(&self) -> f32 {
        *self.dry.lock().unwrap()
    }

    pub fn set_dry(&self, value: f32) {
        *self.dry.lock().unwrap() = value.clamp(0.0, 1.0);
    }

    pub fn width(&self) -> f32 {
        *self.width.lock().unwrap()
    }

    pub fn set_width(&self, value: f32) {
        *self.width.lock().unwrap() = value.clamp(0.0, 1.0);
    }

    pub fn freeze(&self) -> bool {
        *self.freeze.lock().unwrap()
    }

    pub fn set_freeze(&self, value: bool) {
        *self.freeze.lock().unwrap() = value;
    }
}

impl ControlSurface for ReverbControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("room_size", "Room size")
                .with_range(0.0, 1.0)
                .with_default(self.room_size()),
            ControlMeta::number("decay", "Reverb decay time")
                .with_range(0.0, 1.0)
                .with_default(self.decay()),
            ControlMeta::number("damping", "High-frequency damping")
                .with_range(0.0, 1.0)
                .with_default(self.damping()),
            ControlMeta::number("wet", "Wet signal level")
                .with_range(0.0, 1.0)
                .with_default(self.wet()),
            ControlMeta::number("dry", "Dry signal level")
                .with_range(0.0, 1.0)
                .with_default(self.dry()),
            ControlMeta::number("width", "Stereo width")
                .with_range(0.0, 1.0)
                .with_default(self.width()),
            ControlMeta::boolean("freeze", "Infinite hold mode", self.freeze()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "room_size" => Ok(self.room_size().into()),
            "decay" => Ok(self.decay().into()),
            "damping" => Ok(self.damping().into()),
            "wet" => Ok(self.wet().into()),
            "dry" => Ok(self.dry().into()),
            "width" => Ok(self.width().into()),
            "freeze" => Ok(self.freeze().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "room_size" => self.set_room_size(value.as_number()?),
            "decay" => self.set_decay(value.as_number()?),
            "damping" => self.set_damping(value.as_number()?),
            "wet" => self.set_wet(value.as_number()?),
            "dry" => self.set_dry(value.as_number()?),
            "width" => self.set_width(value.as_number()?),
            "freeze" => self.set_freeze(value.as_bool()?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
