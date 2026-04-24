//! Thread-safe controls for the Oscillator.

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

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
    pub(crate) frequency: AtomicF32,
    pub(crate) oscillator_type: AtomicF32,
    pub(crate) fm_amount: AtomicF32,
    pub(crate) am_amount: AtomicF32,
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
            frequency: AtomicF32::new(frequency.max(0.0)),
            oscillator_type: AtomicF32::new(oscillator_type.to_index()),
            fm_amount: AtomicF32::new(fm_amount),
            am_amount: AtomicF32::new(am_amount.clamp(0.0, 1.0)),
        }
    }

    /// Gets the frequency in Hz.
    pub fn frequency(&self) -> f32 {
        self.frequency.load()
    }

    /// Sets the frequency in Hz.
    pub fn set_frequency(&self, value: f32) {
        self.frequency.store(value.max(0.0));
    }

    /// Gets the oscillator type.
    pub fn oscillator_type(&self) -> OscillatorType {
        OscillatorType::from_index(self.oscillator_type.load())
    }

    /// Sets the oscillator type.
    pub fn set_oscillator_type(&self, value: OscillatorType) {
        self.oscillator_type.store(value.to_index());
    }

    /// Gets the FM amount in Hz.
    pub fn fm_amount(&self) -> f32 {
        self.fm_amount.load()
    }

    /// Sets the FM amount in Hz.
    pub fn set_fm_amount(&self, value: f32) {
        self.fm_amount.store(value);
    }

    /// Gets the AM amount (0.0-1.0).
    pub fn am_amount(&self) -> f32 {
        self.am_amount.load()
    }

    /// Sets the AM amount (0.0-1.0).
    pub fn set_am_amount(&self, value: f32) {
        self.am_amount.store(value.clamp(0.0, 1.0));
    }
}

impl ControlSurface for OscillatorControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("frequency", "Frequency in Hz")
                .with_range(20.0, 20000.0)
                .with_default(self.frequency()),
            ControlMeta::string("type", "Waveform type")
                .with_default(self.oscillator_type().as_str())
                .with_options(vec![
                    "sine".to_string(),
                    "square".to_string(),
                    "sawtooth".to_string(),
                    "triangle".to_string(),
                ]),
            ControlMeta::number("fm_amount", "FM modulation depth in Hz")
                .with_range(0.0, 1000.0)
                .with_default(self.fm_amount()),
            ControlMeta::number("am_amount", "AM modulation depth")
                .with_range(0.0, 1.0)
                .with_default(self.am_amount()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "frequency" => Ok(self.frequency().into()),
            "type" => Ok(self.oscillator_type().as_str().into()),
            "fm_amount" => Ok(self.fm_amount().into()),
            "am_amount" => Ok(self.am_amount().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "frequency" => self.set_frequency(value.as_number()?),
            "type" => self.set_oscillator_type(OscillatorType::parse(value.as_string()?)?),
            "fm_amount" => self.set_fm_amount(value.as_number()?),
            "am_amount" => self.set_am_amount(value.as_number()?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
