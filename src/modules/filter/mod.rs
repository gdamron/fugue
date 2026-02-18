//! Filter module for subtractive synthesis.
//!
//! The filter removes or emphasizes frequencies from the input signal,
//! which is the core of subtractive synthesis. Combined with envelope
//! modulation, filters create the classic "analog synth" sound.
//!
//! # Filter Types
//!
//! - **Low-pass (LPF)** - Removes frequencies above cutoff, warm/dark sound
//! - **High-pass (HPF)** - Removes frequencies below cutoff, thin/bright sound
//! - **Band-pass (BPF)** - Passes frequencies around cutoff, vocal/nasal sound
//!
//! # Features
//!
//! - Cutoff frequency: 20 Hz to 20 kHz
//! - Resonance (Q): 0.0 to 1.0 (self-oscillates near 1.0)
//! - CV inputs for cutoff and resonance modulation
//! - State-variable filter design for stability and musicality
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "osc", "type": "oscillator", "config": { "oscillator_type": "sawtooth" } },
//!     { "id": "filter", "type": "filter", "config": { "filter_type": "lowpass", "cutoff": 1000.0, "resonance": 0.5 } },
//!     { "id": "env", "type": "adsr", "config": { "attack": 0.01, "decay": 0.3, "sustain": 0.2, "release": 0.5 } }
//!   ],
//!   "connections": [
//!     { "from": "osc", "from_port": "audio", "to": "filter", "to_port": "audio" },
//!     { "from": "env", "from_port": "envelope", "to": "filter", "to_port": "cutoff_cv" }
//!   ]
//! }
//! ```

use std::any::Any;
use std::f32::consts::PI;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::FilterControls;

mod controls;

/// Filter type selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterType {
    /// Low-pass filter - removes frequencies above cutoff.
    #[default]
    LowPass,
    /// High-pass filter - removes frequencies below cutoff.
    HighPass,
    /// Band-pass filter - passes frequencies around cutoff.
    BandPass,
}

/// Converts filter type to f32 index.
fn filter_type_to_index(filter_type: FilterType) -> f32 {
    match filter_type {
        FilterType::LowPass => 0.0,
        FilterType::HighPass => 1.0,
        FilterType::BandPass => 2.0,
    }
}

/// Converts f32 index to filter type.
fn index_to_filter_type(index: f32) -> FilterType {
    match index.round() as i32 {
        0 => FilterType::LowPass,
        1 => FilterType::HighPass,
        2 => FilterType::BandPass,
        _ => FilterType::LowPass,
    }
}

/// A resonant filter for subtractive synthesis.
///
/// Uses a state-variable filter (SVF) topology, which provides excellent
/// stability and can produce low-pass, high-pass, and band-pass outputs
/// simultaneously.
///
/// # Inputs
///
/// - `audio` - Audio signal to filter
/// - `cutoff` - Cutoff frequency in Hz (overrides control if connected)
/// - `cutoff_cv` - Cutoff modulation (scaled by cv_amount, in Hz)
/// - `resonance` - Resonance modulation (0.0 to 1.0)
///
/// # Outputs
///
/// - `audio` - Filtered audio signal
///
/// # Controls
///
/// - `cutoff` - Base cutoff frequency in Hz (default: 1000.0)
/// - `resonance` - Resonance/Q 0.0-1.0 (default: 0.0)
/// - `type` - Filter type (0=LowPass, 1=HighPass, 2=BandPass)
/// - `cv_amount` - CV modulation depth in Hz (default: 5000.0)
pub struct Filter {
    sample_rate: u32,

    // Controls (shared with FilterControls handle)
    ctrl: FilterControls,

    // State-variable filter state
    low: f32,
    band: f32,

    // Signal inputs
    audio_in: f32,
    cutoff_in: f32,
    cutoff_cv: f32,
    resonance_in: f32,

    // Active flags
    cutoff_active: bool,
    resonance_active: bool,

    // Cached output
    cached_audio: f32,

    // Pull-based processing
    last_processed_sample: u64,
}

impl Filter {
    /// Creates a new filter with default controls.
    pub fn new(sample_rate: u32) -> Self {
        let controls = FilterControls::new(1000.0, 0.0, FilterType::LowPass, 5000.0);
        Self::new_with_controls(sample_rate, controls)
    }

    /// Creates a new filter with the given controls.
    pub fn new_with_controls(sample_rate: u32, controls: FilterControls) -> Self {
        Self {
            sample_rate,
            ctrl: controls,
            low: 0.0,
            band: 0.0,
            audio_in: 0.0,
            cutoff_in: 0.0,
            cutoff_cv: 0.0,
            resonance_in: 0.0,
            cutoff_active: false,
            resonance_active: false,
            cached_audio: 0.0,
            last_processed_sample: 0,
        }
    }

    /// Returns the effective cutoff (signal or control).
    fn effective_cutoff(&self) -> f32 {
        if self.cutoff_active {
            self.cutoff_in
        } else {
            self.ctrl.cutoff()
        }
    }

    /// Returns the effective resonance (signal or control).
    fn effective_resonance(&self) -> f32 {
        if self.resonance_active {
            self.resonance_in
        } else {
            self.ctrl.resonance()
        }
    }

    /// Sets the filter type (legacy API).
    pub fn with_filter_type(self, filter_type: FilterType) -> Self {
        self.ctrl.set_filter_type(filter_type);
        self
    }

    /// Sets the cutoff frequency in Hz (legacy API).
    pub fn with_cutoff(self, cutoff: f32) -> Self {
        self.ctrl.set_cutoff(cutoff);
        self
    }

    /// Sets the resonance (legacy API).
    pub fn with_resonance(self, resonance: f32) -> Self {
        self.ctrl.set_resonance(resonance);
        self
    }

    /// Sets the CV modulation amount in Hz (legacy API).
    pub fn with_cv_amount(self, amount: f32) -> Self {
        self.ctrl.set_cv_amount(amount);
        self
    }

    /// Sets the filter type (legacy API).
    pub fn set_filter_type(&mut self, filter_type: FilterType) {
        self.ctrl.set_filter_type(filter_type);
    }

    /// Sets the cutoff frequency in Hz (legacy API).
    pub fn set_cutoff(&mut self, cutoff: f32) {
        self.ctrl.set_cutoff(cutoff);
    }

    /// Sets the resonance (legacy API).
    pub fn set_resonance(&mut self, resonance: f32) {
        self.ctrl.set_resonance(resonance);
    }

    /// Processes one sample through the filter.
    fn process_sample(&mut self) -> f32 {
        let base_cutoff = self.effective_cutoff();
        let cv_amount = self.ctrl.cv_amount();
        let base_resonance = self.effective_resonance();
        let filter_type = self.ctrl.filter_type();

        // Calculate effective cutoff with CV modulation
        let effective_cutoff = (base_cutoff + self.cutoff_cv * cv_amount).clamp(20.0, 20000.0);

        // Calculate effective resonance
        let effective_resonance = base_resonance.clamp(0.0, 0.99);

        // Convert cutoff to filter coefficient
        let f = (2.0 * (PI * effective_cutoff / self.sample_rate as f32).sin()).min(0.99);

        // Convert resonance to Q factor
        let q = 1.0 - effective_resonance;

        // State-variable filter iteration
        self.low += f * self.band;
        let high = self.audio_in - self.low - q * self.band;
        self.band += f * high;

        // Select output based on filter type
        match filter_type {
            FilterType::LowPass => self.low,
            FilterType::HighPass => high,
            FilterType::BandPass => self.band,
        }
    }

    /// Resets the filter state.
    pub fn reset(&mut self) {
        self.low = 0.0;
        self.band = 0.0;
    }
}

impl Module for Filter {
    fn name(&self) -> &str {
        "Filter"
    }

    fn process(&mut self) -> bool {
        self.cached_audio = self.process_sample();
        true
    }

    fn inputs(&self) -> &[&str] {
        &["audio", "cutoff", "cutoff_cv", "resonance"]
    }

    fn outputs(&self) -> &[&str] {
        &["audio"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "audio" => {
                self.audio_in = value;
                Ok(())
            }
            "cutoff" => {
                self.cutoff_in = value.clamp(20.0, 20000.0);
                self.cutoff_active = true;
                Ok(())
            }
            "cutoff_cv" => {
                self.cutoff_cv = value;
                Ok(())
            }
            "resonance" => {
                self.resonance_in = value.clamp(0.0, 1.0);
                self.resonance_active = true;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.cached_audio),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn reset_inputs(&mut self) {
        self.cutoff_active = false;
        self.resonance_active = false;
    }

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("cutoff", "Cutoff frequency in Hz")
                .with_range(20.0, 20000.0)
                .with_default(1000.0),
            ControlMeta::new("resonance", "Resonance/Q")
                .with_range(0.0, 1.0)
                .with_default(0.0),
            ControlMeta::new("type", "Filter type")
                .with_default(0.0)
                .with_variants(vec![
                    "LowPass".to_string(),
                    "HighPass".to_string(),
                    "BandPass".to_string(),
                ]),
            ControlMeta::new("cv_amount", "CV modulation depth in Hz")
                .with_range(0.0, 10000.0)
                .with_default(5000.0),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "cutoff" => Ok(self.ctrl.cutoff()),
            "resonance" => Ok(self.ctrl.resonance()),
            "type" => Ok(filter_type_to_index(self.ctrl.filter_type())),
            "cv_amount" => Ok(self.ctrl.cv_amount()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "cutoff" => {
                self.ctrl.set_cutoff(value);
                Ok(())
            }
            "resonance" => {
                self.ctrl.set_resonance(value);
                Ok(())
            }
            "type" => {
                self.ctrl.set_filter_type(index_to_filter_type(value));
                Ok(())
            }
            "cv_amount" => {
                self.ctrl.set_cv_amount(value);
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
    }
}

/// Factory for constructing Filter modules from configuration.
pub struct FilterFactory;

impl ModuleFactory for FilterFactory {
    fn type_id(&self) -> &'static str {
        "filter"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let filter_type = parse_filter_type(
            config
                .get("filter_type")
                .and_then(|v| v.as_str())
                .unwrap_or("lowpass"),
        )?;

        let cutoff = config
            .get("cutoff")
            .and_then(|v| v.as_f64())
            .unwrap_or(1000.0) as f32;
        let resonance = config
            .get("resonance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let cv_amount = config
            .get("cv_amount")
            .and_then(|v| v.as_f64())
            .unwrap_or(5000.0) as f32;

        let controls = FilterControls::new(cutoff, resonance, filter_type, cv_amount);
        let filter = Filter::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(filter)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls) as Arc<dyn Any + Send + Sync>,
            )],
            sink: None,
        })
    }
}

/// Parses a filter type string into a FilterType enum.
fn parse_filter_type(s: &str) -> Result<FilterType, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "lowpass" | "lpf" | "low" => Ok(FilterType::LowPass),
        "highpass" | "hpf" | "high" => Ok(FilterType::HighPass),
        "bandpass" | "bpf" | "band" => Ok(FilterType::BandPass),
        _ => Err(format!("Unknown filter type: {}", s).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_controls() {
        let mut filter = Filter::new(44100);

        // Test control metadata
        let controls = filter.controls();
        assert_eq!(controls.len(), 4);
        assert_eq!(controls[0].key, "cutoff");
        assert_eq!(controls[1].key, "resonance");
        assert_eq!(controls[2].key, "type");

        // Test get/set controls
        filter.set_control("cutoff", 2000.0).unwrap();
        assert_eq!(filter.get_control("cutoff").unwrap(), 2000.0);

        filter.set_control("type", 1.0).unwrap(); // HighPass
        assert_eq!(filter.get_control("type").unwrap(), 1.0);
    }

    #[test]
    fn test_filter_lowpass_attenuates_highs() {
        let mut filter = Filter::new(44100)
            .with_filter_type(FilterType::LowPass)
            .with_cutoff(100.0)
            .with_resonance(0.0);

        let mut output_sum = 0.0f32;
        for i in 0..1000 {
            filter.audio_in = if i % 2 == 0 { 1.0 } else { -1.0 };
            filter.process();
            output_sum += filter.get_output("audio").unwrap().abs();
        }

        let avg_output = output_sum / 1000.0;
        assert!(
            avg_output < 0.1,
            "Low-pass should attenuate high frequencies, got avg {}",
            avg_output
        );
    }

    #[test]
    fn test_filter_factory() {
        let factory = FilterFactory;
        assert_eq!(ModuleFactory::type_id(&factory), "filter");

        let config = serde_json::json!({
            "filter_type": "highpass",
            "cutoff": 500.0,
            "resonance": 0.3
        });

        let result = factory.build(44100, &config).unwrap();

        let module = result.module.lock().unwrap();
        assert_eq!(module.name(), "Filter");

        // Check that controls handle is returned
        assert_eq!(result.handles.len(), 1);
        assert_eq!(result.handles[0].0, "controls");
    }
}
