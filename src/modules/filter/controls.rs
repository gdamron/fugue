//! Thread-safe controls for the Filter.

use crate::atomic::AtomicF32;
use crate::traits::{check_listed_control, ControlSurfaceMap};
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
    pub(crate) cutoff_mod_depth: AtomicF32,
}

impl FilterControls {
    /// Creates new filter controls with the given initial values.
    pub fn new(
        cutoff: f32,
        resonance: f32,
        filter_type: FilterType,
        cutoff_mod_depth: f32,
    ) -> Self {
        Self {
            cutoff: AtomicF32::new(cutoff.clamp(20.0, 20000.0)),
            resonance: AtomicF32::new(resonance.clamp(0.0, 1.0)),
            filter_type: AtomicF32::new(filter_type_to_index(filter_type)),
            cutoff_mod_depth: AtomicF32::new(cutoff_mod_depth.max(0.0)),
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
    pub fn cutoff_mod_depth(&self) -> f32 {
        self.cutoff_mod_depth.load()
    }

    /// Sets the CV modulation amount in Hz.
    pub fn set_cutoff_mod_depth(&self, value: f32) {
        self.cutoff_mod_depth.store(value.max(0.0));
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

    /// Parses a filter type name; the control and the config key
    /// `filter_type` share this one spelling set.
    pub(super) fn parse_filter_type(value: &str) -> Result<FilterType, String> {
        match value.to_lowercase().as_str() {
            "lowpass" | "low_pass" | "lpf" | "low" => Ok(FilterType::LowPass),
            "highpass" | "high_pass" | "hpf" | "high" => Ok(FilterType::HighPass),
            "bandpass" | "band_pass" | "bpf" | "band" => Ok(FilterType::BandPass),
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
            ControlMeta::string("filter_type", "Filter type")
                .with_default(Self::filter_type_name(self.filter_type()))
                .with_options(vec![
                    "lowpass".to_string(),
                    "highpass".to_string(),
                    "bandpass".to_string(),
                ]),
            ControlMeta::number("cutoff_mod_depth", "CV modulation depth in Hz")
                .with_range(0.0, 20000.0)
                .with_default(self.cutoff_mod_depth()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "cutoff" => Ok(self.cutoff().into()),
            "resonance" => Ok(self.resonance().into()),
            "filter_type" => Ok(Self::filter_type_name(self.filter_type()).into()),
            "cutoff_mod_depth" => Ok(self.cutoff_mod_depth().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "cutoff" => self.set_cutoff(value.as_number()?),
            "resonance" => self.set_resonance(value.as_number()?),
            "filter_type" => self.set_filter_type(Self::parse_filter_type(value.as_string()?)?),
            "cutoff_mod_depth" => self.set_cutoff_mod_depth(value.as_number()?),
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
            "filter_type" => Self::parse_filter_type(value.as_string()?).map(drop),
            _ => check_listed_control(&self.controls(), key, value),
        }
    }
}
