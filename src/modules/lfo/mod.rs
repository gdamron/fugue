//! Low Frequency Oscillator (LFO) module for modulation.
//!
//! The LFO generates sub-audio frequency waveforms used to modulate other
//! parameters like pitch (vibrato), amplitude (tremolo), or filter cutoff.
//!
//! # Features
//!
//! - Multiple waveforms: sine, triangle, square, sawtooth
//! - Rate range: 0.01 Hz to 20 Hz (typical LFO range)
//! - Bipolar output (-1.0 to +1.0) for FM/pitch modulation
//! - Unipolar output (0.0 to +1.0) for amplitude modulation
//! - Sync input to reset phase on trigger
//! - Rate modulation input for complex rhythmic effects
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "lfo", "type": "lfo", "config": { "rate": 5.0, "waveform": "sine" } },
//!     { "id": "osc", "type": "oscillator", "config": { "frequency": 440.0, "frequency_mod_depth": 20.0 } }
//!   ],
//!   "connections": [
//!     { "from": "lfo", "from_port": "bipolar", "to": "osc", "to_port": "frequency_mod" }
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
use crate::modules::OscillatorType;
use crate::traits::ControlMeta;
use crate::Module;

use self::controls::{RATE, RATE_MOD_DEPTH, TABLE, WAVEFORM};

mod controls;
mod inputs;
mod outputs;

/// Low Frequency Oscillator for modulation.
///
/// Like a slow-moving oscillator that creates rhythmic changes to other
/// parameters. In Eurorack terms, this is a modulation source that you'd
/// patch into CV inputs.
///
/// # Outputs
///
/// - `bipolar` - Bipolar signal (-1.0 to +1.0), ideal for pitch/FM modulation
/// - `unipolar` - Unipolar signal (0.0 to +1.0), ideal for amplitude modulation
///
/// # Inputs
///
/// - `sync` - Trigger input (rising edge resets phase to 0)
/// - `rate_mod` - Rate modulation (adds Hz to `rate`, scaled by `rate_mod_depth`)
///
/// # Controls
///
/// - `rate` - LFO rate in Hz (default: 1.0)
/// - `rate_mod_depth` - Hz added per unit of `rate_mod` (default: 1.0)
/// - `waveform` - Waveform type (0=Sine, 1=Square, 2=Sawtooth, 3=Triangle)
pub struct Lfo {
    phase: f32,
    sample_rate: u32,

    // Controls, applied on the thread running the LFO.
    rate: f32,
    rate_mod_depth: f32,
    waveform: OscillatorType,
    cells: Arc<ControlCells>,

    // Input values
    inputs: inputs::LfoInputs,
    prev_sync: f32,

    // Cached outputs
    outputs: outputs::LfoOutputs,
}

impl Lfo {
    /// Creates a new LFO with default controls.
    pub fn new(sample_rate: u32) -> Self {
        Self::with_cells(
            sample_rate,
            Arc::new(ControlCells::new(controls::defaults())),
        )
    }

    /// An LFO holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let mut lfo = Self {
            phase: 0.0,
            sample_rate,
            rate: 1.0,
            rate_mod_depth: 1.0,
            waveform: OscillatorType::Sine,
            cells,
            inputs: inputs::LfoInputs::new(),
            prev_sync: 0.0,
            outputs: outputs::LfoOutputs::new(),
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = lfo.cells.load(index) {
                let _ = apply_declared(&mut lfo, index, value);
            }
        }
        lfo
    }

    /// Sets the waveform type (legacy API).
    pub fn with_waveform(mut self, waveform: OscillatorType) -> Self {
        self.set_waveform(waveform);
        self
    }

    /// Sets the rate in Hz (legacy API).
    pub fn with_rate(mut self, rate: f32) -> Self {
        self.set_rate(rate);
        self
    }

    /// Sets the waveform type (legacy API).
    pub fn set_waveform(&mut self, waveform: OscillatorType) {
        let _ = apply_declared(self, WAVEFORM, RtValue::U32(waveform.position()));
    }

    /// Sets the rate in Hz (legacy API).
    pub fn set_rate(&mut self, rate: f32) {
        let _ = apply_declared(self, RATE, RtValue::F32(rate));
    }

    /// Resets the phase to zero.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Generates the sample for frame `i` based on current waveform and phase.
    fn generate(&mut self, i: usize) -> f32 {
        // Check for sync trigger (rising edge detection)
        if self.inputs.sync(i) > 0.5 && self.prev_sync <= 0.5 {
            self.phase = 0.0;
        }
        self.prev_sync = self.inputs.sync(i);

        let base_rate = self.rate;
        let rate_mod_depth = self.rate_mod_depth;
        let waveform = self.waveform;

        // Calculate effective rate with modulation
        let effective_freq =
            (base_rate + self.inputs.rate_mod(i) * rate_mod_depth).clamp(0.001, 100.0);

        // Generate waveform (bipolar: -1.0 to +1.0)
        let sample = match waveform {
            OscillatorType::Sine => (self.phase * 2.0 * PI).sin(),
            OscillatorType::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            OscillatorType::Sawtooth => 2.0 * self.phase - 1.0,
            OscillatorType::Triangle => 4.0 * (self.phase - 0.5).abs() - 1.0,
        };

        // Advance phase
        self.phase += effective_freq / self.sample_rate as f32;
        self.phase -= self.phase.floor();

        sample
    }
}

impl Module for Lfo {
    fn name(&self) -> &str {
        "Lfo"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let out = self.generate(i);
            self.outputs.set_bipolar(i, out);
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

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let applied = match (control, value) {
            (RATE, RtValue::F32(rate)) => {
                self.rate = rate.clamp(0.001, 100.0);
                RtValue::F32(self.rate)
            }
            (RATE_MOD_DEPTH, RtValue::F32(depth)) => {
                self.rate_mod_depth = depth.max(0.0);
                RtValue::F32(self.rate_mod_depth)
            }
            (WAVEFORM, RtValue::U32(position)) => {
                self.waveform = OscillatorType::from_position(position).ok_or(Refusal::Invalid)?;
                value
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

/// Factory for constructing LFO modules from configuration.
pub struct LfoFactory;

const TYPE_ID: &str = "lfo";
const RATE_KEY: ConfigKey = ConfigKey::float("rate");
const RATE_MOD_DEPTH_KEY: ConfigKey = ConfigKey::float("rate_mod_depth");

impl ModuleFactory for LfoFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[RATE_KEY, RATE_MOD_DEPTH_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let rate = reader.float(&RATE_KEY)?.unwrap_or(1.0);
        let rate_mod_depth = reader.float(&RATE_MOD_DEPTH_KEY)?.unwrap_or(1.0);

        let cells = Arc::new(ControlCells::new(controls::defaults()));
        cells.publish(RATE, RtValue::F32(rate));
        cells.publish(RATE_MOD_DEPTH, RtValue::F32(rate_mod_depth));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        apply_control_keys(&surface, config, |key| key == "waveform")?;
        let lfo = Lfo::with_cells(sample_rate, cells);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(lfo)),
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
    fn test_lfo_controls() {
        let mut lfo = Lfo::new(1000);

        // Test control metadata
        let controls = lfo.controls();
        assert_eq!(controls.len(), 3);
        assert_eq!(controls[0].key, "rate");
        assert_eq!(controls[1].key, "rate_mod_depth");
        assert_eq!(controls[2].key, "waveform");

        // Test get/set controls
        lfo.set_control("rate", 5.0).unwrap();
        assert_eq!(lfo.get_control("rate").unwrap(), 5.0);

        lfo.set_control("waveform", 2.0).unwrap(); // Sawtooth
        assert_eq!(lfo.get_control("waveform").unwrap(), 2.0);
    }

    #[test]
    fn test_lfo_sine_output_range() {
        let mut lfo = Lfo::new(1000);
        lfo.set_rate(10.0);

        let mut min = f32::MAX;
        let mut max = f32::MIN;

        for _ in 0..100 {
            lfo.process(1);
            let out = lfo.get_output("bipolar").unwrap();
            let out_uni = lfo.get_output("unipolar").unwrap();

            min = min.min(out);
            max = max.max(out);

            assert!((0.0..=1.0).contains(&out_uni));
        }

        assert!(min < -0.9, "min was {}", min);
        assert!(max > 0.9, "max was {}", max);
    }

    #[test]
    fn rate_mod_adds_hz_scaled_by_its_depth() {
        let render = |depth: f32, rate_mod: f32| {
            let mut lfo = Lfo::new(1000);
            lfo.set_rate(2.0);
            lfo.set_control("rate_mod_depth", depth).unwrap();
            lfo.set_input("rate_mod", rate_mod).unwrap();
            (0..100)
                .map(|_| {
                    lfo.process(1);
                    lfo.get_output("bipolar").unwrap()
                })
                .collect::<Vec<f32>>()
        };

        // Depth scales the input: 1 Hz of `rate_mod` at depth 1.0 is the
        // same added Hz as 2 units at depth 0.5.
        assert_eq!(render(1.0, 1.0), render(0.5, 2.0));
        // Depth 0 ignores the input.
        assert_eq!(render(0.0, 50.0), render(0.0, 0.0));
        assert_ne!(render(1.0, 50.0), render(0.0, 50.0));
    }

    #[test]
    fn test_lfo_factory() {
        let factory = LfoFactory;
        assert_eq!(ModuleFactory::type_id(&factory), "lfo");

        let config = serde_json::json!({
            "rate": 5.0,
            "waveform": "triangle"
        });

        let result = factory.build(44100, &config).unwrap();

        let module = result.module.module();
        assert_eq!(module.name(), "Lfo");

        assert!(result.control_surface.is_some());
    }

    #[test]
    fn a_waveform_change_keeps_the_phase() {
        let mut lfo = Lfo::new(1000);
        lfo.set_rate(10.0);
        lfo.process(25);
        let phase = lfo.phase;
        assert!(phase > 0.0);

        lfo.set_control("waveform", 2.0).unwrap();
        assert_eq!(lfo.phase, phase, "the phase is not reset");
        lfo.process(1);
        // A sawtooth read at the phase the sine had reached.
        let expected = 2.0 * phase - 1.0;
        assert_eq!(lfo.get_output("bipolar").unwrap(), expected);
    }

    #[test]
    fn every_waveform_spelling_still_loads() {
        let registry = crate::ModuleRegistry::default();
        for (spelled, read) in [
            ("sine", "sine"),
            ("Square", "square"),
            ("sawtooth", "sawtooth"),
            ("SAW", "sawtooth"),
            ("saw", "sawtooth"),
            ("triangle", "triangle"),
            ("tri", "triangle"),
        ] {
            let config = serde_json::json!({ "waveform": spelled });
            let built = registry.build("lfo", 48_000, &config).unwrap();
            let surface = built.control_surface.unwrap();
            assert_eq!(surface.get_control("waveform").unwrap(), read.into());
            surface.set_control("waveform", spelled.into()).unwrap();
        }
        let config = serde_json::json!({ "waveform": "wobble" });
        assert!(registry.build("lfo", 48_000, &config).is_err());
    }

    #[test]
    fn controls_clamp_as_they_always_did() {
        let mut lfo = Lfo::new(1000);
        lfo.set_control("rate", 500.0).unwrap();
        assert_eq!(lfo.get_control("rate").unwrap(), 100.0);
        lfo.set_control("rate", 0.0).unwrap();
        assert_eq!(lfo.get_control("rate").unwrap(), 0.001);
        lfo.set_control("rate_mod_depth", -3.0).unwrap();
        assert_eq!(lfo.get_control("rate_mod_depth").unwrap(), 0.0);
        assert!(lfo.set_control("rate", f32::NAN).is_err());
    }
}
