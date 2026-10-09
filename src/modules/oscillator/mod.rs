//! Oscillator module for waveform generation.

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
use std::f32::consts::PI;

use self::controls::{AMPLITUDE_MOD_DEPTH, FREQUENCY, FREQUENCY_MOD_DEPTH, TABLE, WAVEFORM};
pub use self::waveform::OscillatorType;

mod controls;
mod inputs;
mod outputs;
mod waveform;

/// Sine approximation over the full cycle via a shaped parabolic fit.
///
/// Input `phase` is in [0, 1). Max error is about 0.12% while preserving the
/// sine zero crossings and extrema.
#[inline(always)]
fn fast_sine(phase: f32) -> f32 {
    let x = phase * 2.0 * PI;
    let x = if x > PI { x - 2.0 * PI } else { x };

    let y = (4.0 / PI) * x + (-4.0 / (PI * PI)) * x * x.abs();
    0.225 * (y * y.abs() - y) + y
}

/// Factory for constructing Oscillator modules from configuration.
pub struct OscillatorFactory;

const TYPE_ID: &str = "oscillator";
const FREQUENCY_KEY: ConfigKey = ConfigKey::float("frequency");
const FREQUENCY_MOD_DEPTH_KEY: ConfigKey = ConfigKey::float("frequency_mod_depth");
const AMPLITUDE_MOD_DEPTH_KEY: ConfigKey = ConfigKey::float("amplitude_mod_depth");

impl ModuleFactory for OscillatorFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[
            FREQUENCY_KEY,
            FREQUENCY_MOD_DEPTH_KEY,
            AMPLITUDE_MOD_DEPTH_KEY,
        ]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let frequency = reader.float(&FREQUENCY_KEY)?.unwrap_or(440.0);
        let fm_amount = reader.float(&FREQUENCY_MOD_DEPTH_KEY)?.unwrap_or(0.0);
        let am_amount = reader.float(&AMPLITUDE_MOD_DEPTH_KEY)?.unwrap_or(0.0);
        let initial = [frequency, fm_amount, am_amount];
        built(sample_rate, initial, config, |key| key == "waveform")
    }
}

/// An oscillator and its surface, starting from `frequency`, `fm_amount`
/// and `am_amount`, with `config`'s control keys that `applied` names
/// written over them (by name, as a client would).
pub(crate) fn built(
    sample_rate: u32,
    [frequency, fm_amount, am_amount]: [f32; 3],
    config: &serde_json::Value,
    applied: impl Fn(&str) -> bool,
) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
    let cells = Arc::new(ControlCells::new(controls::defaults()));
    cells.publish(FREQUENCY, RtValue::F32(frequency));
    cells.publish(FREQUENCY_MOD_DEPTH, RtValue::F32(fm_amount));
    cells.publish(AMPLITUDE_MOD_DEPTH, RtValue::F32(am_amount));
    let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
    apply_control_keys(&surface, config, applied)?;
    let osc = Oscillator::with_cells(sample_rate, cells);
    Ok(ModuleBuildResult {
        module: GraphModule::Module(Box::new(osc)),
        handles: Vec::new(),
        control_surface: Some(Arc::new(surface)),
        sink: None,
    })
}

/// A waveform generator that produces audio signals.
///
/// # Inputs
/// - `frequency`: Frequency in Hz (overrides control if connected)
/// - `frequency_mod`: Frequency modulation signal (scaled by frequency_mod_depth)
/// - `amplitude_mod`: Amplitude modulation signal (scaled by amplitude_mod_depth)
///
/// # Outputs
/// - `audio`: Generated audio waveform
///
/// # Controls
/// - `frequency`: Base frequency in Hz (default: 440.0)
/// - `waveform`: Waveform (0=Sine, 1=Square, 2=Sawtooth, 3=Triangle)
/// - `frequency_mod_depth`: Frequency modulation depth in Hz (default: 0.0)
/// - `amplitude_mod_depth`: Amplitude modulation depth 0.0-1.0 (default: 0.0)
pub struct Oscillator {
    phase: f32,
    sample_rate: u32,

    // Controls, applied on the thread running the oscillator.
    frequency: f32,
    waveform: OscillatorType,
    fm_amount: f32,
    am_amount: f32,
    cells: Arc<ControlCells>,

    // Signal inputs
    inputs: inputs::OscillatorInputs,

    // Cached output
    outputs: outputs::OscillatorOutputs,
}

impl Oscillator {
    /// Creates a new oscillator at 440 Hz with no modulation.
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        let mut osc = Self::with_cells(
            sample_rate,
            Arc::new(ControlCells::new(controls::defaults())),
        );
        osc.set_type(osc_type);
        osc
    }

    /// An oscillator holding what `cells` hold, applied (so clamped).
    fn with_cells(sample_rate: u32, cells: Arc<ControlCells>) -> Self {
        let mut osc = Self {
            phase: 0.0,
            sample_rate,
            frequency: 440.0,
            waveform: OscillatorType::Sine,
            fm_amount: 0.0,
            am_amount: 0.0,
            cells,
            inputs: inputs::OscillatorInputs::new(),
            outputs: outputs::OscillatorOutputs::new(),
        };
        for index in 0..TABLE.len() {
            let index = ControlIndex(index as u16);
            if let Some(value) = osc.cells.load(index) {
                let _ = apply_declared(&mut osc, index, value);
            }
        }
        osc
    }

    /// Returns the effective frequency (signal or control) at frame 0.
    #[cfg(test)]
    fn effective_frequency(&self) -> f32 {
        self.inputs.frequency(0, self.frequency)
    }

    /// Sets the oscillator frequency in Hz (legacy API).
    pub fn with_frequency(mut self, freq: f32) -> Self {
        self.set_frequency(freq);
        self
    }

    /// Sets the frequency modulation depth in Hz (legacy API).
    pub fn with_fm_amount(mut self, amount: f32) -> Self {
        self.set_fm_amount(amount);
        self
    }

    /// Sets the amplitude modulation depth (legacy API).
    pub fn with_am_amount(mut self, amount: f32) -> Self {
        self.set_am_amount(amount);
        self
    }

    /// Sets the oscillator frequency in Hz (legacy API).
    pub fn set_frequency(&mut self, freq: f32) {
        let _ = apply_declared(self, FREQUENCY, RtValue::F32(freq));
    }

    /// Changes the waveform type (legacy API).
    pub fn set_type(&mut self, osc_type: OscillatorType) {
        let _ = apply_declared(self, WAVEFORM, RtValue::U32(osc_type.position()));
    }

    /// Sets the frequency modulation depth in Hz (legacy API).
    pub fn set_fm_amount(&mut self, amount: f32) {
        let _ = apply_declared(self, FREQUENCY_MOD_DEPTH, RtValue::F32(amount));
    }

    /// Sets the amplitude modulation depth (legacy API).
    pub fn set_am_amount(&mut self, amount: f32) {
        let _ = apply_declared(self, AMPLITUDE_MOD_DEPTH, RtValue::F32(amount));
    }

    /// Generates frame `i` from scalar controls sampled for this process call
    /// and modulation inputs that remain audio-rate.
    #[inline(always)]
    fn generate_sample_with(
        &mut self,
        i: usize,
        control_frequency: f32,
        fm_amount: f32,
        am_amount: f32,
        osc_type: OscillatorType,
    ) -> f32 {
        let base_freq = self.inputs.frequency(i, control_frequency);
        let modulated_freq = base_freq + (self.inputs.fm(i) * fm_amount);

        let sample = match osc_type {
            OscillatorType::Sine => fast_sine(self.phase),
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
            let normalized_amp = (self.inputs.am(i) + 1.0) * 0.5;
            1.0 - am_amount + (normalized_amp * am_amount)
        } else {
            1.0
        };

        sample * amp_scale
    }

    /// Generates the sample for frame `i`, reading per-sample modulation inputs.
    pub(crate) fn generate_sample(&mut self, i: usize) -> f32 {
        let control_frequency = if self.inputs.frequency_connected() {
            0.0
        } else {
            self.frequency
        };
        self.generate_sample_with(
            i,
            control_frequency,
            self.fm_amount,
            self.am_amount,
            self.waveform,
        )
    }

    /// Resets the oscillator phase to zero.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Generates the next sample (legacy API).
    pub fn next_sample(&mut self) -> f32 {
        self.generate_sample(0)
    }
}

impl Module for Oscillator {
    fn name(&self) -> &str {
        "Oscillator"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Scalar controls are block-rate. Signal/FM/AM inputs remain
        // audio-rate, and feedback groups call process(1).
        let control_frequency = if self.inputs.frequency_connected() {
            0.0
        } else {
            self.frequency
        };
        let fm_amount = self.fm_amount;
        let am_amount = self.am_amount;
        let osc_type = self.waveform;

        let mut i = 0;
        while i < frames {
            let audio =
                self.generate_sample_with(i, control_frequency, fm_amount, am_amount, osc_type);
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
            (FREQUENCY, RtValue::F32(frequency)) => {
                self.frequency = frequency.max(0.0);
                RtValue::F32(self.frequency)
            }
            (WAVEFORM, RtValue::U32(position)) => {
                self.waveform = OscillatorType::from_position(position).ok_or(Refusal::Invalid)?;
                value
            }
            (FREQUENCY_MOD_DEPTH, RtValue::F32(amount)) => {
                self.fm_amount = amount;
                value
            }
            (AMPLITUDE_MOD_DEPTH, RtValue::F32(amount)) => {
                self.am_amount = amount.clamp(0.0, 1.0);
                RtValue::F32(self.am_amount)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fast_sine_cardinal_points() {
        let epsilon = 0.000_001;

        assert!(fast_sine(0.0).abs() < epsilon);
        assert!((fast_sine(0.25) - 1.0).abs() < epsilon);
        assert!(fast_sine(0.5).abs() < epsilon);
        assert!((fast_sine(0.75) + 1.0).abs() < epsilon);
    }

    #[test]
    fn test_fast_sine_matches_reference_with_bounded_error() {
        let max_error = (0..1024)
            .map(|i| {
                let phase = i as f32 / 1024.0;
                (fast_sine(phase) - (phase * 2.0 * PI).sin()).abs()
            })
            .fold(0.0, f32::max);

        assert!(
            max_error < 0.001_3,
            "max sine approximation error {max_error}"
        );
    }

    #[test]
    fn test_fast_sine_has_no_half_cycle_discontinuity() {
        let before = fast_sine(0.5 - 0.000_1);
        let at = fast_sine(0.5);
        let after = fast_sine(0.5 + 0.000_1);

        assert!(before.abs() < 0.001);
        assert!(at.abs() < 0.000_001);
        assert!(after.abs() < 0.001);
    }

    #[test]
    fn test_oscillator_controls() {
        let mut osc = Oscillator::new(44100, OscillatorType::Sine);

        // Test control metadata
        let controls = osc.controls();
        assert_eq!(controls.len(), 4);
        assert_eq!(controls[0].key, "frequency");
        assert_eq!(controls[1].key, "waveform");

        // Test get/set controls
        osc.set_control("frequency", 880.0).unwrap();
        assert_eq!(osc.get_control("frequency").unwrap(), 880.0);

        osc.set_control("waveform", 2.0).unwrap(); // Sawtooth
        assert_eq!(osc.get_control("waveform").unwrap(), 2.0);

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
        assert_eq!(osc.effective_frequency(), 880.0);

        // After disconnecting the frequency port, should use control again
        osc.set_input_connected(0, false);
        assert_eq!(osc.effective_frequency(), 440.0);
    }

    #[test]
    fn test_connected_frequency_remains_audio_rate() {
        let sample_rate = 44_100;
        let mut osc = Oscillator::new(sample_rate, OscillatorType::Sawtooth);
        let frequency = osc.input_block_mut(0);
        frequency[..3].copy_from_slice(&[0.0, sample_rate as f32 / 4.0, 0.0]);
        osc.set_input_connected(0, true);

        osc.process(3);

        assert_eq!(&osc.output_block(0)[..3], &[-1.0, -1.0, -0.5]);
    }
}
