//! Low Frequency Oscillator (LFO) module for modulation.
//!
//! The LFO generates sub-audio frequency waveforms used to modulate other
//! parameters like pitch (vibrato), amplitude (tremolo), or filter cutoff.
//!
//! # Features
//!
//! - Multiple waveforms: sine, triangle, square, sawtooth
//! - Frequency range: 0.01 Hz to 20 Hz (typical LFO range)
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
//!     { "id": "lfo", "type": "lfo", "config": { "frequency": 5.0, "waveform": "sine" } },
//!     { "id": "osc", "type": "oscillator", "config": { "frequency": 440.0, "fm_amount": 20.0 } }
//!   ],
//!   "connections": [
//!     { "from": "lfo", "from_port": "out", "to": "osc", "to_port": "fm" }
//!   ]
//! }
//! ```

use std::any::Any;
use std::f32::consts::PI;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
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
/// - `out` - Bipolar signal (-1.0 to +1.0), ideal for pitch/FM modulation
/// - `out_uni` - Unipolar signal (0.0 to +1.0), ideal for amplitude modulation
///
/// # Inputs
///
/// - `sync` - Trigger input (rising edge resets phase to 0)
/// - `rate` - Frequency modulation (adds to base frequency)
///
/// # Controls
///
/// - `frequency` - LFO rate in Hz (default: 1.0)
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

    // Pull-based processing
    last_processed_sample: u64,
}

impl Lfo {
    /// Creates a new LFO with default controls.
    pub fn new(sample_rate: u32) -> Self {
        let controls = LfoControls::new(1.0, OscillatorType::Sine);
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
            last_processed_sample: 0,
        }
    }

    /// Sets the waveform type (legacy API).
    pub fn with_waveform(self, waveform: OscillatorType) -> Self {
        self.ctrl.set_waveform(waveform);
        self
    }

    /// Sets the frequency in Hz (legacy API).
    pub fn with_frequency(self, freq: f32) -> Self {
        self.ctrl.set_frequency(freq);
        self
    }

    /// Sets the waveform type (legacy API).
    pub fn set_waveform(&mut self, waveform: OscillatorType) {
        self.ctrl.set_waveform(waveform);
    }

    /// Sets the frequency in Hz (legacy API).
    pub fn set_frequency(&mut self, freq: f32) {
        self.ctrl.set_frequency(freq);
    }

    /// Resets the phase to zero.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Generates the next sample based on current waveform and phase.
    fn generate(&mut self) -> f32 {
        // Check for sync trigger (rising edge detection)
        if self.inputs.sync() > 0.5 && self.prev_sync <= 0.5 {
            self.phase = 0.0;
        }
        self.prev_sync = self.inputs.sync();

        let base_freq = self.ctrl.frequency();
        let waveform = self.ctrl.waveform();

        // Calculate effective frequency with modulation
        let effective_freq = (base_freq + self.inputs.rate()).clamp(0.001, 100.0);

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

    fn process(&mut self) -> bool {
        let out = self.generate();
        self.outputs.set_bipolar(out);
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("frequency", "LFO rate in Hz")
                .with_range(0.001, 100.0)
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
            "frequency" => Ok(self.ctrl.frequency()),
            "waveform" => Ok(waveform_to_index(self.ctrl.waveform())),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "frequency" => {
                self.ctrl.set_frequency(value);
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

impl ModuleFactory for LfoFactory {
    fn type_id(&self) -> &'static str {
        "lfo"
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

        let frequency = config
            .get("frequency")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;

        let controls = LfoControls::new(frequency, waveform);
        let lfo = Lfo::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(lfo)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls) as Arc<dyn Any + Send + Sync>,
            )],
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
        assert_eq!(controls.len(), 2);
        assert_eq!(controls[0].key, "frequency");
        assert_eq!(controls[1].key, "waveform");

        // Test get/set controls
        lfo.set_control("frequency", 5.0).unwrap();
        assert_eq!(lfo.get_control("frequency").unwrap(), 5.0);

        lfo.set_control("waveform", 2.0).unwrap(); // Sawtooth
        assert_eq!(lfo.get_control("waveform").unwrap(), 2.0);
    }

    #[test]
    fn test_lfo_sine_output_range() {
        let mut lfo = Lfo::new(1000);
        lfo.set_frequency(10.0);

        let mut min = f32::MAX;
        let mut max = f32::MIN;

        for _ in 0..100 {
            lfo.process();
            let out = lfo.get_output("out").unwrap();
            let out_uni = lfo.get_output("out_uni").unwrap();

            min = min.min(out);
            max = max.max(out);

            assert!(out_uni >= 0.0 && out_uni <= 1.0);
        }

        assert!(min < -0.9, "min was {}", min);
        assert!(max > 0.9, "max was {}", max);
    }

    #[test]
    fn test_lfo_factory() {
        let factory = LfoFactory;
        assert_eq!(ModuleFactory::type_id(&factory), "lfo");

        let config = serde_json::json!({
            "frequency": 5.0,
            "waveform": "triangle"
        });

        let result = factory.build(44100, &config).unwrap();

        let module = result.module.lock().unwrap();
        assert_eq!(module.name(), "Lfo");

        // Check that controls handle is returned
        assert_eq!(result.handles.len(), 1);
        assert_eq!(result.handles[0].0, "controls");
    }
}
