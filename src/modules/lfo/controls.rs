//! Thread-safe controls for the LFO.

use std::sync::{Arc, Mutex};

use crate::modules::OscillatorType;
use crate::{ControlMeta, ControlSurface, ControlValue};

/// Thread-safe controls for the LFO.
///
/// All fields are wrapped in `Arc<Mutex<_>>` for real-time adjustment
/// from any thread while audio is playing.
///
/// # Example
///
/// ```rust,ignore
/// let controls: LfoControls = handles.get("lfo1.controls").unwrap();
///
/// // Adjust LFO in real-time
/// controls.set_frequency(5.0);
/// controls.set_waveform(OscillatorType::Triangle);
/// ```
#[derive(Clone)]
pub struct LfoControls {
    pub(crate) frequency: Arc<Mutex<f32>>,
    pub(crate) waveform: Arc<Mutex<OscillatorType>>,
}

impl LfoControls {
    /// Creates new LFO controls with the given initial values.
    pub fn new(frequency: f32, waveform: OscillatorType) -> Self {
        Self {
            frequency: Arc::new(Mutex::new(frequency.clamp(0.001, 100.0))),
            waveform: Arc::new(Mutex::new(waveform)),
        }
    }

    /// Gets the frequency in Hz.
    pub fn frequency(&self) -> f32 {
        *self.frequency.lock().unwrap()
    }

    /// Sets the frequency in Hz.
    pub fn set_frequency(&self, value: f32) {
        *self.frequency.lock().unwrap() = value.clamp(0.001, 100.0);
    }

    /// Gets the waveform type.
    pub fn waveform(&self) -> OscillatorType {
        *self.waveform.lock().unwrap()
    }

    /// Sets the waveform type.
    pub fn set_waveform(&self, value: OscillatorType) {
        *self.waveform.lock().unwrap() = value;
    }
}

impl ControlSurface for LfoControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("frequency", "LFO rate in Hz")
                .with_range(0.001, 100.0)
                .with_default(self.frequency()),
            ControlMeta::string("waveform", "Waveform type")
                .with_default(self.waveform().as_str())
                .with_options(vec![
                    "sine".to_string(),
                    "square".to_string(),
                    "sawtooth".to_string(),
                    "triangle".to_string(),
                ]),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "frequency" => Ok(self.frequency().into()),
            "waveform" => Ok(self.waveform().as_str().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "frequency" => self.set_frequency(value.as_number()?),
            "waveform" => self.set_waveform(OscillatorType::parse(value.as_string()?)?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
