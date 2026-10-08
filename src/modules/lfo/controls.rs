//! Thread-safe controls for the LFO.

use crate::atomic::AtomicF32;
use crate::modules::OscillatorType;
use crate::traits::{check_listed_control, ControlSurfaceMap};
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
/// controls.set_rate(5.0);
/// controls.set_waveform(OscillatorType::Triangle);
/// ```
#[derive(Clone)]
pub struct LfoControls {
    pub(crate) rate: AtomicF32,
    pub(crate) rate_mod_depth: AtomicF32,
    pub(crate) waveform: AtomicF32,
}

impl LfoControls {
    /// Creates new LFO controls with the given initial values.
    pub fn new(rate: f32, waveform: OscillatorType, rate_mod_depth: f32) -> Self {
        Self {
            rate: AtomicF32::new(rate.clamp(0.001, 100.0)),
            rate_mod_depth: AtomicF32::new(rate_mod_depth.max(0.0)),
            waveform: AtomicF32::new(waveform.to_index()),
        }
    }

    /// Gets the rate in Hz.
    pub fn rate(&self) -> f32 {
        self.rate.load()
    }

    /// Sets the rate in Hz.
    pub fn set_rate(&self, value: f32) {
        self.rate.store(value.clamp(0.001, 100.0));
    }

    /// Gets the rate modulation depth in Hz per unit of `rate_mod`.
    pub fn rate_mod_depth(&self) -> f32 {
        self.rate_mod_depth.load()
    }

    /// Sets the rate modulation depth in Hz per unit of `rate_mod`.
    pub fn set_rate_mod_depth(&self, value: f32) {
        self.rate_mod_depth.store(value.max(0.0));
    }

    /// Gets the waveform type.
    pub fn waveform(&self) -> OscillatorType {
        OscillatorType::from_index(self.waveform.load())
    }

    /// Sets the waveform type.
    pub fn set_waveform(&self, value: OscillatorType) {
        self.waveform.store(value.to_index());
    }
}

impl ControlSurface for LfoControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("rate", "LFO rate in Hz")
                .with_range(0.001, 100.0)
                .with_default(self.rate()),
            ControlMeta::number("rate_mod_depth", "Rate modulation depth in Hz")
                .with_range(0.0, 100.0)
                .with_default(self.rate_mod_depth()),
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
            "rate" => Ok(self.rate().into()),
            "rate_mod_depth" => Ok(self.rate_mod_depth().into()),
            "waveform" => Ok(self.waveform().as_str().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "rate" => self.set_rate(value.as_number()?),
            "rate_mod_depth" => self.set_rate_mod_depth(value.as_number()?),
            "waveform" => self.set_waveform(OscillatorType::parse(value.as_string()?)?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        match key {
            "waveform" => OscillatorType::parse(value.as_string()?).map(drop),
            _ => check_listed_control(&self.controls(), key, value),
        }
    }
}
