//! Thread-safe controls for the Filter.

use crate::atomic::AtomicF32;
use crate::{ControlMeta, ControlSurface, ControlValue};

use super::FilterType;

fn filter_type_to_index(filter_type: FilterType) -> f32 {
    match filter_type {
        FilterType::LowPass => 0.0,
        FilterType::HighPass => 1.0,
        FilterType::BandPass => 2.0,
    }
}

fn index_to_filter_type(index: f32) -> FilterType {
    match index.round() as i32 {
        0 => FilterType::LowPass,
        1 => FilterType::HighPass,
        2 => FilterType::BandPass,
        _ => FilterType::LowPass,
    }
}

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
    pub(crate) cutoff: AtomicF32,
    pub(crate) resonance: AtomicF32,
    pub(crate) filter_type: AtomicF32,
    pub(crate) cv_amount: AtomicF32,
}

impl FilterControls {
    /// Creates new filter controls with the given initial values.
    pub fn new(cutoff: f32, resonance: f32, filter_type: FilterType, cv_amount: f32) -> Self {
        Self {
            cutoff: AtomicF32::new(cutoff.clamp(20.0, 20000.0)),
            resonance: AtomicF32::new(resonance.clamp(0.0, 1.0)),
            filter_type: AtomicF32::new(filter_type_to_index(filter_type)),
            cv_amount: AtomicF32::new(cv_amount.max(0.0)),
        }
    }

    /// Gets the cutoff frequency in Hz.
    pub fn cutoff(&self) -> f32 {
        self.cutoff.load()
    }

    /// Sets the cutoff frequency in Hz.
    pub fn set_cutoff(&self, value: f32) {
        self.cutoff.store(value.clamp(20.0, 20000.0));
    }

    /// Gets the resonance (0.0-1.0).
    pub fn resonance(&self) -> f32 {
        self.resonance.load()
    }

    /// Sets the resonance (0.0-1.0).
    pub fn set_resonance(&self, value: f32) {
        self.resonance.store(value.clamp(0.0, 1.0));
    }

    /// Gets the filter type.
    pub fn filter_type(&self) -> FilterType {
        index_to_filter_type(self.filter_type.load())
    }

    /// Sets the filter type.
    pub fn set_filter_type(&self, value: FilterType) {
        self.filter_type.store(filter_type_to_index(value));
    }

    /// Gets the CV modulation amount in Hz.
    pub fn cv_amount(&self) -> f32 {
        self.cv_amount.load()
    }

    /// Sets the CV modulation amount in Hz.
    pub fn set_cv_amount(&self, value: f32) {
        self.cv_amount.store(value.max(0.0));
    }
}

impl FilterControls {
    fn filter_type_name(value: FilterType) -> &'static str {
        match value {
            FilterType::LowPass => "lowpass",
            FilterType::HighPass => "highpass",
            FilterType::BandPass => "bandpass",
        }
    }

    fn parse_filter_type(value: &str) -> Result<FilterType, String> {
        match value.to_lowercase().as_str() {
            "lowpass" | "low_pass" | "lpf" => Ok(FilterType::LowPass),
            "highpass" | "high_pass" | "hpf" => Ok(FilterType::HighPass),
            "bandpass" | "band_pass" | "bpf" => Ok(FilterType::BandPass),
            _ => Err(format!("Unknown filter type: {}", value)),
        }
    }
}

impl ControlSurface for FilterControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::number("cutoff", "Cutoff frequency in Hz")
                .with_range(20.0, 20000.0)
                .with_default(self.cutoff()),
            ControlMeta::number("resonance", "Resonance/Q")
                .with_range(0.0, 1.0)
                .with_default(self.resonance()),
            ControlMeta::string("type", "Filter type")
                .with_default(Self::filter_type_name(self.filter_type()))
                .with_options(vec![
                    "lowpass".to_string(),
                    "highpass".to_string(),
                    "bandpass".to_string(),
                ]),
            ControlMeta::number("cv_amount", "CV modulation depth in Hz")
                .with_range(0.0, 20000.0)
                .with_default(self.cv_amount()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "cutoff" => Ok(self.cutoff().into()),
            "resonance" => Ok(self.resonance().into()),
            "type" => Ok(Self::filter_type_name(self.filter_type()).into()),
            "cv_amount" => Ok(self.cv_amount().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "cutoff" => self.set_cutoff(value.as_number()?),
            "resonance" => self.set_resonance(value.as_number()?),
            "type" => self.set_filter_type(Self::parse_filter_type(value.as_string()?)?),
            "cv_amount" => self.set_cv_amount(value.as_number()?),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
