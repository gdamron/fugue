//! Oscillator module for waveform generation.

use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::Module;
use std::f32::consts::PI;

pub use self::controls::OscillatorControls;
pub use self::waveform::OscillatorType;

mod controls;
mod waveform;

/// Factory for constructing Oscillator modules from configuration.
pub struct OscillatorFactory;

impl ModuleFactory for OscillatorFactory {
    fn type_id(&self) -> &'static str {
        "oscillator"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let osc_type = parse_oscillator_type(
            config
                .get("oscillator_type")
                .and_then(|v| v.as_str())
                .unwrap_or("sine"),
        )?;

        let frequency = config
            .get("frequency")
            .and_then(|v| v.as_f64())
            .unwrap_or(440.0) as f32;
        let fm_amount = config
            .get("fm_amount")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        let am_amount = config
            .get("am_amount")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;

        let controls = OscillatorControls::new(frequency, osc_type, fm_amount, am_amount);
        let osc = Oscillator::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(osc)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls) as Arc<dyn Any + Send + Sync>,
            )],
            sink: None,
        })
    }
}

/// Parses an oscillator type string into an OscillatorType enum.
fn parse_oscillator_type(s: &str) -> Result<OscillatorType, Box<dyn std::error::Error>> {
    match s.to_lowercase().as_str() {
        "sine" => Ok(OscillatorType::Sine),
        "square" => Ok(OscillatorType::Square),
        "sawtooth" | "saw" => Ok(OscillatorType::Sawtooth),
        "triangle" | "tri" => Ok(OscillatorType::Triangle),
        _ => Err(format!("Unknown oscillator type: {}", s).into()),
    }
}



/// A waveform generator that produces audio signals.
///
/// # Inputs
/// - `frequency`: Frequency in Hz (overrides control if connected)
/// - `fm`: Frequency modulation signal (scaled by fm_amount)
/// - `am`: Amplitude modulation signal (scaled by am_amount)
///
/// # Outputs
/// - `audio`: Generated audio waveform
///
/// # Controls
/// - `frequency`: Base frequency in Hz (default: 440.0)
/// - `type`: Waveform type (0=Sine, 1=Square, 2=Sawtooth, 3=Triangle)
/// - `fm_amount`: FM modulation depth in Hz (default: 0.0)
/// - `am_amount`: AM modulation depth 0.0-1.0 (default: 0.0)
pub struct Oscillator {
    phase: f32,
    sample_rate: u32,

    // Controls (shared with OscillatorControls handle)
    ctrl: OscillatorControls,

    // Signal inputs
    frequency_in: f32,
    fm_in: f32,
    am_in: f32,

    // Active flags
    frequency_active: bool,

    // Cached output
    cached_audio: f32,
    last_processed_sample: u64,
}

impl Oscillator {
    /// Creates a new oscillator with default controls.
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        let controls = OscillatorControls::new(440.0, osc_type, 0.0, 0.0);
        Self::new_with_controls(sample_rate, controls)
    }

    /// Creates a new oscillator with the given controls.
    pub fn new_with_controls(sample_rate: u32, controls: OscillatorControls) -> Self {
        Self {
            phase: 0.0,
            sample_rate,
            ctrl: controls,
            frequency_in: 0.0,
            fm_in: 0.0,
            am_in: 0.0,
            frequency_active: false,
            cached_audio: 0.0,
            last_processed_sample: 0,
        }
    }

    /// Returns the effective frequency (signal or control).
    fn effective_frequency(&self) -> f32 {
        if self.frequency_active {
            self.frequency_in
        } else {
            self.ctrl.frequency()
        }
    }

    /// Sets the oscillator frequency in Hz (legacy API).
    pub fn with_frequency(self, freq: f32) -> Self {
        self.ctrl.set_frequency(freq);
        self
    }

    /// Sets the frequency modulation depth in Hz (legacy API).
    pub fn with_fm_amount(self, amount: f32) -> Self {
        self.ctrl.set_fm_amount(amount);
        self
    }

    /// Sets the amplitude modulation depth (legacy API).
    pub fn with_am_amount(self, amount: f32) -> Self {
        self.ctrl.set_am_amount(amount);
        self
    }

    /// Sets the oscillator frequency in Hz (legacy API).
    pub fn set_frequency(&mut self, freq: f32) {
        self.ctrl.set_frequency(freq);
    }

    /// Changes the waveform type (legacy API).
    pub fn set_type(&mut self, osc_type: OscillatorType) {
        self.ctrl.set_oscillator_type(osc_type);
    }

    /// Sets the frequency modulation depth in Hz (legacy API).
    pub fn set_fm_amount(&mut self, amount: f32) {
        self.ctrl.set_fm_amount(amount);
    }

    /// Sets the amplitude modulation depth (legacy API).
    pub fn set_am_amount(&mut self, amount: f32) {
        self.ctrl.set_am_amount(amount);
    }

    /// Generates a sample with the given modulation values.
    fn generate_sample_with_modulation(&mut self, freq_mod: f32, amp_mod: f32) -> f32 {
        let base_freq = self.effective_frequency();
        let fm_amount = self.ctrl.fm_amount();
        let am_amount = self.ctrl.am_amount();
        let osc_type = self.ctrl.oscillator_type();

        let modulated_freq = base_freq + (freq_mod * fm_amount);

        let sample = match osc_type {
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

        self.phase += modulated_freq / self.sample_rate as f32;
        self.phase %= 1.0;

        let amp_scale = if am_amount > 0.0 {
            let normalized_amp = (amp_mod + 1.0) * 0.5;
            1.0 - am_amount + (normalized_amp * am_amount)
        } else {
            1.0
        };

        sample * amp_scale
    }

    pub(crate) fn generate_sample(&mut self) -> f32 {
        self.generate_sample_with_modulation(self.fm_in, self.am_in)
    }

    /// Resets the oscillator phase to zero.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Generates the next sample (legacy API).
    pub fn next_sample(&mut self) -> f32 {
        self.generate_sample()
    }
}

impl Module for Oscillator {
    fn name(&self) -> &str {
        "Oscillator"
    }

    fn process(&mut self) -> bool {
        self.cached_audio = self.generate_sample();
        true
    }

    fn inputs(&self) -> &[&str] {
        &["frequency", "fm", "am"]
    }

    fn outputs(&self) -> &[&str] {
        &["audio"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "frequency" => {
                self.frequency_in = value;
                self.frequency_active = true;
                Ok(())
            }
            "fm" => {
                self.fm_in = value;
                Ok(())
            }
            "am" => {
                self.am_in = value;
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
        self.frequency_active = false;
        // fm_in and am_in don't have control fallbacks, they're pure modulation
    }

    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::new("frequency", "Frequency in Hz")
                .with_range(20.0, 20000.0)
                .with_default(440.0),
            ControlMeta::new("type", "Waveform type")
                .with_default(0.0)
                .with_variants(vec![
                    "Sine".to_string(),
                    "Square".to_string(),
                    "Sawtooth".to_string(),
                    "Triangle".to_string(),
                ]),
            ControlMeta::new("fm_amount", "FM modulation depth in Hz")
                .with_range(0.0, 1000.0)
                .with_default(0.0),
            ControlMeta::new("am_amount", "AM modulation depth")
                .with_range(0.0, 1.0)
                .with_default(0.0),
        ]
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "frequency" => Ok(self.ctrl.frequency()),
            "type" => Ok(self.ctrl.oscillator_type().to_index()),
            "fm_amount" => Ok(self.ctrl.fm_amount()),
            "am_amount" => Ok(self.ctrl.am_amount()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "frequency" => {
                self.ctrl.set_frequency(value);
                Ok(())
            }
            "type" => {
                self.ctrl.set_oscillator_type(OscillatorType::from_index(value));
                Ok(())
            }
            "fm_amount" => {
                self.ctrl.set_fm_amount(value);
                Ok(())
            }
            "am_amount" => {
                self.ctrl.set_am_amount(value);
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oscillator_controls() {
        let mut osc = Oscillator::new(44100, OscillatorType::Sine);

        // Test control metadata
        let controls = osc.controls();
        assert_eq!(controls.len(), 4);
        assert_eq!(controls[0].key, "frequency");
        assert_eq!(controls[1].key, "type");

        // Test get/set controls
        osc.set_control("frequency", 880.0).unwrap();
        assert_eq!(osc.get_control("frequency").unwrap(), 880.0);

        osc.set_control("type", 2.0).unwrap(); // Sawtooth
        assert_eq!(osc.get_control("type").unwrap(), 2.0);

        // Test invalid control
        assert!(osc.get_control("invalid").is_err());
    }

    #[test]
    fn test_oscillator_signal_overrides_control() {
        let mut osc = Oscillator::new(44100, OscillatorType::Sine);

        // Set control frequency
        osc.set_control("frequency", 440.0).unwrap();

        // Signal input should override
        osc.set_input("frequency", 880.0).unwrap();
        assert!(osc.frequency_active);
        assert_eq!(osc.effective_frequency(), 880.0);

        // After reset_inputs, should use control again
        osc.reset_inputs();
        assert!(!osc.frequency_active);
        assert_eq!(osc.effective_frequency(), 440.0);
    }
}
