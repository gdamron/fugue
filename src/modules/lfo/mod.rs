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

use std::any::Any;
use std::f32::consts::PI;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};
use crate::modules::OscillatorType;
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::LfoControls;

mod controls;
mod inputs;
mod outputs;

/// Converts oscillator type to f32 index.
fn waveform_to_index(waveform: OscillatorType) -> f32 {
    match waveform {
        OscillatorType::Sine => 0.0,
        OscillatorType::Square => 1.0,
        OscillatorType::Sawtooth => 2.0,
        OscillatorType::Triangle => 3.0,
    }
}

/// Converts f32 index to oscillator type.
fn index_to_waveform(index: f32) -> OscillatorType {
    match index.round() as i32 {
        0 => OscillatorType::Sine,
        1 => OscillatorType::Square,
        2 => OscillatorType::Sawtooth,
        3 => OscillatorType::Triangle,
        _ => OscillatorType::Sine,
    }
}

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

    // Controls (shared with LfoControls handle)
    ctrl: LfoControls,

    // Input values
    inputs: inputs::LfoInputs,
    prev_sync: f32,

    // Cached outputs
    outputs: outputs::LfoOutputs,
}

impl Lfo {
    /// Creates a new LFO with default controls.
    pub fn new(sample_rate: u32) -> Self {
        let controls = LfoControls::new(1.0, OscillatorType::Sine, 1.0);
        Self::new_with_controls(sample_rate, controls)
    }

    /// Creates a new LFO with the given controls.
    pub fn new_with_controls(sample_rate: u32, controls: LfoControls) -> Self {
        Self {
            phase: 0.0,
            sample_rate,
            ctrl: controls,
            inputs: inputs::LfoInputs::new(),
            prev_sync: 0.0,
            outputs: outputs::LfoOutputs::new(),
        }
    }

    /// Sets the waveform type (legacy API).
    pub fn with_waveform(self, waveform: OscillatorType) -> Self {
        self.ctrl.set_waveform(waveform);
        self
    }

    /// Sets the rate in Hz (legacy API).
    pub fn with_rate(self, rate: f32) -> Self {
        self.ctrl.set_rate(rate);
        self
    }

    /// Sets the waveform type (legacy API).
    pub fn set_waveform(&mut self, waveform: OscillatorType) {
        self.ctrl.set_waveform(waveform);
    }

    /// Sets the rate in Hz (legacy API).
    pub fn set_rate(&mut self, rate: f32) {
        self.ctrl.set_rate(rate);
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

        let base_rate = self.ctrl.rate();
        let rate_mod_depth = self.ctrl.rate_mod_depth();
        let waveform = self.ctrl.waveform();

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
        self.phase %= 1.0;

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

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("rate", "LFO rate in Hz")
                .with_range(0.001, 100.0)
                .with_default(1.0),
            ControlMeta::new("rate_mod_depth", "Rate modulation depth in Hz")
                .with_range(0.0, 100.0)
                .with_default(1.0),
            ControlMeta::new("waveform", "Waveform type")
                .with_default(0.0)
                .with_variants(vec![
                    "Sine".to_string(),
                    "Square".to_string(),
                    "Sawtooth".to_string(),
                    "Triangle".to_string(),
                ]),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "rate" => Ok(self.ctrl.rate()),
            "rate_mod_depth" => Ok(self.ctrl.rate_mod_depth()),
            "waveform" => Ok(waveform_to_index(self.ctrl.waveform())),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "rate" => {
                self.ctrl.set_rate(value);
                Ok(())
            }
            "rate_mod_depth" => {
                self.ctrl.set_rate_mod_depth(value);
                Ok(())
            }
            "waveform" => {
                self.ctrl.set_waveform(index_to_waveform(value));
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
    }
}

/// Factory for constructing LFO modules from configuration.
pub struct LfoFactory;

const TYPE_ID: &str = "lfo";
const RATE: ConfigKey = ConfigKey::float("rate");
const RATE_MOD_DEPTH: ConfigKey = ConfigKey::float("rate_mod_depth");

impl ModuleFactory for LfoFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const { &[RATE, RATE_MOD_DEPTH, ConfigKey::text("waveform")] }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let waveform = parse_waveform(
            config
                .get("waveform")
                .and_then(|v| v.as_str())
                .unwrap_or("sine"),
        )?;

        let reader = ConfigReader::new(TYPE_ID, config);
        let rate = reader.float(&RATE)?.unwrap_or(1.0);
        let rate_mod_depth = reader.float(&RATE_MOD_DEPTH)?.unwrap_or(1.0);

        let controls = LfoControls::new(rate, waveform, rate_mod_depth);
        let lfo = Lfo::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(lfo)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

/// Parses a waveform string into an OscillatorType enum.
fn parse_waveform(s: &str) -> Result<OscillatorType, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "sine" => Ok(OscillatorType::Sine),
        "square" => Ok(OscillatorType::Square),
        "sawtooth" | "saw" => Ok(OscillatorType::Sawtooth),
        "triangle" | "tri" => Ok(OscillatorType::Triangle),
        _ => Err(format!("Unknown waveform type: {}", s).into()),
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

        // Check that controls handle is returned
        assert_eq!(result.handles.len(), 1);
        assert_eq!(result.handles[0].0, "controls");
    }
}
