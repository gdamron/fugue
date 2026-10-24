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
//!     { "id": "osc", "type": "oscillator", "config": { "waveform": "sawtooth" } },
//!     { "id": "filter", "type": "filter", "config": { "filter_type": "lowpass", "cutoff": 1000.0, "resonance": 0.5 } },
//!     { "id": "env", "type": "adsr", "config": { "attack": 0.01, "decay": 0.3, "sustain": 0.2, "release": 0.5 } }
//!   ],
//!   "connections": [
//!     { "from": "osc", "from_port": "audio", "to": "filter", "to_port": "audio" },
//!     { "from": "env", "from_port": "envelope", "to": "filter", "to_port": "cutoff_mod" }
//!   ]
//! }
//! ```

use std::f32::consts::PI;
use std::sync::Arc;

use crate::control_request::{
    apply_declared, local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable,
    Refusal, RtValue,
};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::traits::ControlMeta;
use crate::Module;

use self::controls::{CUTOFF, CUTOFF_MOD_DEPTH, FILTER_TYPE, RESONANCE, TABLE};

mod controls;
mod inputs;
mod outputs;

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

impl FilterType {
    /// Its position among the `filter_type` options.
    fn position(self) -> u32 {
        match self {
            FilterType::LowPass => 0,
            FilterType::HighPass => 1,
            FilterType::BandPass => 2,
        }
    }

    /// The type at `position` among the `filter_type` options.
    fn from_position(position: u32) -> Option<Self> {
        match position {
            0 => Some(FilterType::LowPass),
            1 => Some(FilterType::HighPass),
            2 => Some(FilterType::BandPass),
            _ => None,
        }
    }
}

/// Polynomial approximation of tan(x) for x in [0, ~0.85].
///
/// Uses the degree-7 Taylor series in Horner form. Max error < 0.2% for
/// x < 0.75 rad (cutoff < 10.5 kHz at 44.1 kHz), which is inaudible for
/// filter coefficient calculations. Roughly 10× faster than libm tan().
#[inline(always)]
fn fast_tan(x: f32) -> f32 {
    let x2 = x * x;
    x * (1.0 + x2 * (0.333_333_34 + x2 * (0.133_333_34 + x2 * 0.053_968_26)))
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
/// - `cutoff_mod` - Cutoff modulation (scaled by cutoff_mod_depth, in Hz)
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
/// - `filter_type` - Filter type (0=LowPass, 1=HighPass, 2=BandPass)
/// - `cutoff_mod_depth` - CV modulation depth in Hz (default: 5000.0)
pub struct Filter {
    sample_rate: u32,

    // Controls, applied on the thread running the filter.
    cutoff: f32,
    resonance: f32,
    filter_type: FilterType,
    cutoff_mod_depth: f32,
    cells: Arc<ControlCells>,

    // TPT SVF integrator state
    ic1eq: f32,
    ic2eq: f32,

    // Signal inputs
    inputs: inputs::FilterInputs,

    // Cached output
    outputs: outputs::FilterOutputs,

    // Cached SVF coefficients — recomputed only when effective cutoff or
    // resonance changes. Sentinel -1.0 forces computation on first call.
    coeff_cutoff: f32,
    coeff_resonance: f32,
    coeff_g: f32,
    coeff_k: f32,
    coeff_a1: f32,
    coeff_a2: f32,
    coeff_a3: f32,
}

impl Filter {
    /// Creates a new filter with default controls.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_cells(
            sample_rate,
            Arc::new(ControlCells::new(controls::defaults())),
        )
    }

    /// A filter holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let mut filter = Self {
            sample_rate,
            cutoff: 1000.0,
            resonance: 0.0,
            filter_type: FilterType::LowPass,
            cutoff_mod_depth: 5000.0,
            cells,
            ic1eq: 0.0,
            ic2eq: 0.0,
            inputs: inputs::FilterInputs::new(),
            outputs: outputs::FilterOutputs::new(),
            coeff_cutoff: -1.0,
            coeff_resonance: -1.0,
            coeff_g: 0.0,
            coeff_k: 0.0,
            coeff_a1: 0.0,
            coeff_a2: 0.0,
            coeff_a3: 0.0,
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = filter.cells.load(index) {
                let _ = apply_declared(&mut filter, index, value);
            }
        }
        filter
    }

    /// Sets the filter type (legacy API).
    pub fn with_filter_type(mut self, filter_type: FilterType) -> Self {
        self.set_filter_type(filter_type);
        self
    }

    /// Sets the cutoff frequency in Hz (legacy API).
    pub fn with_cutoff(mut self, cutoff: f32) -> Self {
        self.set_cutoff(cutoff);
        self
    }

    /// Sets the resonance (legacy API).
    pub fn with_resonance(mut self, resonance: f32) -> Self {
        self.set_resonance(resonance);
        self
    }

    /// Sets the CV modulation amount in Hz (legacy API).
    pub fn with_cutoff_mod_depth(mut self, amount: f32) -> Self {
        let _ = apply_declared(&mut self, CUTOFF_MOD_DEPTH, RtValue::F32(amount));
        self
    }

    /// Sets the filter type (legacy API).
    pub fn set_filter_type(&mut self, filter_type: FilterType) {
        let _ = apply_declared(self, FILTER_TYPE, RtValue::U32(filter_type.position()));
    }

    /// Sets the cutoff frequency in Hz (legacy API).
    pub fn set_cutoff(&mut self, cutoff: f32) {
        let _ = apply_declared(self, CUTOFF, RtValue::F32(cutoff));
    }

    /// Sets the resonance (legacy API).
    pub fn set_resonance(&mut self, resonance: f32) {
        let _ = apply_declared(self, RESONANCE, RtValue::F32(resonance));
    }

    /// Processes one sample through the filter using the trapezoidal integrated
    /// SVF (Andrew Simper / Cytomic). Unconditionally stable at all frequencies.
    #[inline(always)]
    fn process_sample_with(
        &mut self,
        i: usize,
        control_cutoff: f32,
        control_resonance: f32,
        cutoff_mod_depth: f32,
        filter_type: FilterType,
    ) -> f32 {
        let base_cutoff = self.inputs.cutoff(i, control_cutoff);
        let base_resonance = self.inputs.resonance(i, control_resonance);
        let effective_cutoff =
            (base_cutoff + self.inputs.cutoff_mod(i) * cutoff_mod_depth).clamp(20.0, 20000.0);
        let effective_resonance = base_resonance.clamp(0.0, 0.99);

        // Recompute SVF coefficients only when cutoff or resonance changes.
        // During ADSR sustain or slow LFO phases the values are stable and we
        // skip the fast_tan() call entirely.
        if effective_cutoff != self.coeff_cutoff || effective_resonance != self.coeff_resonance {
            let g = fast_tan(PI * effective_cutoff / self.sample_rate as f32);
            let k = 2.0 * (1.0 - effective_resonance);
            self.coeff_g = g;
            self.coeff_k = k;
            self.coeff_a1 = 1.0 / (1.0 + g * (g + k));
            self.coeff_a2 = g * self.coeff_a1;
            self.coeff_a3 = g * self.coeff_a2;
            self.coeff_cutoff = effective_cutoff;
            self.coeff_resonance = effective_resonance;
        }

        let input = self.inputs.audio(i);

        // TPT SVF tick
        let v3 = input - self.ic2eq;
        let v1 = self.coeff_a1 * self.ic1eq + self.coeff_a2 * v3;
        let v2 = self.ic2eq + self.coeff_a2 * self.ic1eq + self.coeff_a3 * v3;

        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;

        match filter_type {
            FilterType::LowPass => v2,
            FilterType::HighPass => input - self.coeff_k * v1 - v2,
            FilterType::BandPass => v1,
        }
    }

    /// Resets the filter state.
    pub fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }
}

impl Module for Filter {
    fn name(&self) -> &str {
        "Filter"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Scalar controls are block-rate. Cutoff, cutoff CV, and resonance
        // signal inputs remain audio-rate, and feedback groups call process(1).
        let control_cutoff = if self.inputs.cutoff_connected() {
            0.0
        } else {
            self.cutoff
        };
        let control_resonance = if self.inputs.resonance_connected() {
            0.0
        } else {
            self.resonance
        };
        let cutoff_mod_depth = self.cutoff_mod_depth;
        let filter_type = self.filter_type;

        let mut i = 0;
        while i < frames {
            let audio = self.process_sample_with(
                i,
                control_cutoff,
                control_resonance,
                cutoff_mod_depth,
                filter_type,
            );
            self.outputs.set(i, audio);
            i += 1;
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn set_input_connected(&mut self, index: usize, connected: bool) {
        self.inputs.set_connected(index, connected);
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let applied = match (control, value) {
            (CUTOFF, RtValue::F32(cutoff)) => {
                self.cutoff = cutoff.clamp(20.0, 20000.0);
                RtValue::F32(self.cutoff)
            }
            (RESONANCE, RtValue::F32(resonance)) => {
                self.resonance = resonance.clamp(0.0, 1.0);
                RtValue::F32(self.resonance)
            }
            (FILTER_TYPE, RtValue::U32(position)) => {
                self.filter_type = FilterType::from_position(position).ok_or(Refusal::Invalid)?;
                value
            }
            (CUTOFF_MOD_DEPTH, RtValue::F32(depth)) => {
                self.cutoff_mod_depth = depth.max(0.0);
                RtValue::F32(self.cutoff_mod_depth)
            }
            _ => return Err(Refusal::Unsupported),
        };
        Ok(applied)
    }

    fn controls(&self) -> Vec<ControlMeta> {
        local_controls(self)
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        local_get(self, key)
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        local_set(self, key, value)
    }
}

/// Factory for constructing Filter modules from configuration.
pub struct FilterFactory;

const TYPE_ID: &str = "filter";
const CUTOFF_KEY: ConfigKey = ConfigKey::float("cutoff");
const RESONANCE_KEY: ConfigKey = ConfigKey::float("resonance");
const CUTOFF_MOD_DEPTH_KEY: ConfigKey = ConfigKey::float("cutoff_mod_depth");

impl ModuleFactory for FilterFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[CUTOFF_KEY, RESONANCE_KEY, CUTOFF_MOD_DEPTH_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let cutoff = reader.float(&CUTOFF_KEY)?.unwrap_or(1000.0);
        let resonance = reader.float(&RESONANCE_KEY)?.unwrap_or(0.0);
        let cutoff_mod_depth = reader.float(&CUTOFF_MOD_DEPTH_KEY)?.unwrap_or(5000.0);

        let cells = Arc::new(ControlCells::new(controls::defaults()));
        cells.publish(CUTOFF, RtValue::F32(cutoff));
        cells.publish(RESONANCE, RtValue::F32(resonance));
        cells.publish(CUTOFF_MOD_DEPTH, RtValue::F32(cutoff_mod_depth));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        // Only text names a filter type; anything else leaves the default,
        // as it always has.
        let named = config
            .get("filter_type")
            .is_some_and(|value| value.is_string());
        apply_control_keys(&surface, config, |key| named && key == "filter_type")?;
        let filter = Filter::with_cells(sample_rate, cells);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(filter)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
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
        assert_eq!(controls[2].key, "filter_type");

        // Test get/set controls
        filter.set_control("cutoff", 2000.0).unwrap();
        assert_eq!(filter.get_control("cutoff").unwrap(), 2000.0);

        filter.set_control("filter_type", 1.0).unwrap(); // HighPass
        assert_eq!(filter.get_control("filter_type").unwrap(), 1.0);
    }

    #[test]
    fn test_filter_lowpass_attenuates_highs() {
        let mut filter = Filter::new(44100)
            .with_filter_type(FilterType::LowPass)
            .with_cutoff(100.0)
            .with_resonance(0.0);

        let mut output_sum = 0.0f32;
        for i in 0..1000 {
            filter
                .set_input("audio", if i % 2 == 0 { 1.0 } else { -1.0 })
                .unwrap();
            filter.process(1);
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

        let module = result.module.module();
        assert_eq!(module.name(), "Filter");

        assert!(result.control_surface.is_some());
    }

    #[test]
    fn config_and_control_share_filter_type_and_modulation_names() {
        let factory = FilterFactory;
        // The control's spellings (`low_pass`) are config spellings too, so a
        // written control value rebuilds from the config.
        let config = serde_json::json!({
            "filter_type": "band_pass",
            "cutoff_mod_depth": 1234.0
        });
        let result = factory.build(44100, &config).unwrap();
        let surface = result.control_surface.unwrap();
        assert_eq!(
            surface.get_control("filter_type").unwrap(),
            "bandpass".into()
        );
        assert_eq!(
            surface.get_control("cutoff_mod_depth").unwrap(),
            1234.0.into()
        );
        assert!(surface.get_control("type").is_err());
        assert!(surface.get_control("cv_amount").is_err());

        let mut filter = Filter::new(44100);
        assert!(filter.set_input("cutoff_mod", 1.0).is_ok());
        assert!(filter.set_input("cutoff_cv", 1.0).is_err());
    }

    #[test]
    fn every_filter_type_spelling_still_loads() {
        let registry = crate::ModuleRegistry::default();
        for (spelled, read) in [
            ("lowpass", "lowpass"),
            ("low_pass", "lowpass"),
            ("LPF", "lowpass"),
            ("low", "lowpass"),
            ("highpass", "highpass"),
            ("high_pass", "highpass"),
            ("hpf", "highpass"),
            ("High", "highpass"),
            ("bandpass", "bandpass"),
            ("band_pass", "bandpass"),
            ("bpf", "bandpass"),
            ("band", "bandpass"),
        ] {
            let config = serde_json::json!({ "filter_type": spelled });
            let built = registry.build("filter", 48_000, &config).unwrap();
            let surface = built.control_surface.unwrap();
            assert_eq!(surface.get_control("filter_type").unwrap(), read.into());
            surface.set_control("filter_type", spelled.into()).unwrap();
            assert_eq!(surface.get_control("filter_type").unwrap(), read.into());
        }
        let config = serde_json::json!({ "filter_type": "notch" });
        assert!(registry.build("filter", 48_000, &config).is_err());
        // Anything but text still leaves the default.
        for config in [
            serde_json::json!({ "filter_type": null }),
            serde_json::json!({ "filter_type": 2 }),
        ] {
            let built = registry.build("filter", 48_000, &config).unwrap();
            let surface = built.control_surface.unwrap();
            assert_eq!(
                surface.get_control("filter_type").unwrap(),
                "lowpass".into()
            );
        }
    }

    #[test]
    fn there_is_no_type_alias_for_the_filter_type() {
        // Neither a document's config nor a control write has ever taken
        // `type` for `filter_type`; the declared table does not start to.
        let registry = crate::ModuleRegistry::default();
        let config = serde_json::json!({ "type": "highpass" });
        assert!(registry.build("filter", 48_000, &config).is_err());
        let built = registry
            .build("filter", 48_000, &serde_json::json!({}))
            .unwrap();
        let surface = built.control_surface.unwrap();
        assert!(surface.set_control("type", "highpass".into()).is_err());
    }

    #[test]
    fn controls_clamp_as_they_always_did() {
        let mut filter = Filter::new(44100);
        filter.set_control("cutoff", 5.0).unwrap();
        assert_eq!(filter.get_control("cutoff").unwrap(), 20.0);
        filter.set_control("cutoff", 99999.0).unwrap();
        assert_eq!(filter.get_control("cutoff").unwrap(), 20000.0);
        filter.set_control("resonance", 3.0).unwrap();
        assert_eq!(filter.get_control("resonance").unwrap(), 1.0);
        filter.set_control("cutoff_mod_depth", -1.0).unwrap();
        assert_eq!(filter.get_control("cutoff_mod_depth").unwrap(), 0.0);
        assert!(filter.set_control("filter_type", 3.0).is_err());
        assert!(filter.set_control("cutoff", f32::NAN).is_err());
    }
}
