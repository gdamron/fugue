//! Oscillators for waveform generation.
//!
//! - [`Oscillator`] - Basic waveform generator with FM/AM support
//! - [`OscillatorType`] - Waveform shapes (sine, square, saw, triangle)
//! - [`ModulatedOscillator`] - Oscillator with external modulation inputs
//! - [`ModulationInputs`] - FM/AM modulation values

mod modulation;
mod oscillator_type;

pub use modulation::{ModulatedOscillator, ModulationInputs};
pub use oscillator_type::OscillatorType;

use crate::module::{Generator, ModularModule, Module, Processor};
use crate::signal::{AudioSignal, FrequencySignal};
use std::f32::consts::PI;

/// A waveform generator that produces audio signals.
///
/// Can operate as a [`Generator`] with a fixed frequency, or as a
/// [`Processor`] that accepts [`FrequencySignal`] input. Supports
/// frequency modulation (FM) and amplitude modulation (AM).
pub struct Oscillator {
    osc_type: OscillatorType,
    frequency: f32,
    phase: f32,
    sample_rate: u32,
    fm_amount: f32,
    am_amount: f32,
}

impl Oscillator {
    /// Creates a new oscillator with the given sample rate and waveform type.
    ///
    /// Defaults to 440 Hz with no modulation.
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        Self {
            osc_type,
            frequency: 440.0,
            phase: 0.0,
            sample_rate,
            fm_amount: 0.0,
            am_amount: 0.0,
        }
    }

    /// Sets the oscillator frequency in Hz.
    pub fn with_frequency(mut self, freq: f32) -> Self {
        self.frequency = freq;
        self
    }

    /// Sets the frequency modulation depth in Hz.
    pub fn with_fm_amount(mut self, amount: f32) -> Self {
        self.fm_amount = amount;
        self
    }

    /// Sets the amplitude modulation depth (0.0 to 1.0).
    pub fn with_am_amount(mut self, amount: f32) -> Self {
        self.am_amount = amount.clamp(0.0, 1.0);
        self
    }

    /// Sets the oscillator frequency in Hz.
    pub fn set_frequency(&mut self, freq: f32) {
        self.frequency = freq;
    }

    /// Changes the waveform type.
    pub fn set_type(&mut self, osc_type: OscillatorType) {
        self.osc_type = osc_type;
    }

    /// Sets the frequency modulation depth in Hz.
    pub fn set_fm_amount(&mut self, amount: f32) {
        self.fm_amount = amount;
    }

    /// Sets the amplitude modulation depth (0.0 to 1.0).
    pub fn set_am_amount(&mut self, amount: f32) {
        self.am_amount = amount.clamp(0.0, 1.0);
    }

    /// Generates a sample with the given modulation values.
    ///
    /// - `freq_mod`: Frequency modulation signal (scaled by `fm_amount`)
    /// - `amp_mod`: Amplitude modulation signal in range [-1, 1] (scaled by `am_amount`)
    pub fn generate_sample_with_modulation(&mut self, freq_mod: f32, amp_mod: f32) -> f32 {
        let modulated_freq = self.frequency + (freq_mod * self.fm_amount);

        let sample = match self.osc_type {
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

        let amp_scale = if self.am_amount > 0.0 {
            let normalized_amp = (amp_mod + 1.0) * 0.5;
            1.0 - self.am_amount + (normalized_amp * self.am_amount)
        } else {
            1.0
        };

        sample * amp_scale
    }

    pub(crate) fn generate_sample(&mut self) -> f32 {
        self.generate_sample_with_modulation(0.0, 0.0)
    }

    /// Resets the oscillator phase to zero.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Generates the next sample (legacy API, prefer using as Generator).
    pub fn next_sample(&mut self) -> f32 {
        self.generate_sample()
    }
}

impl Module for Oscillator {
    fn process(&mut self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "Oscillator"
    }
}

impl Generator<AudioSignal> for Oscillator {
    fn output(&mut self) -> AudioSignal {
        AudioSignal::new(self.generate_sample())
    }
}

impl Processor<FrequencySignal, AudioSignal> for Oscillator {
    fn process_signal(&mut self, input: FrequencySignal) -> AudioSignal {
        self.set_frequency(input.hz);
        AudioSignal::new(self.generate_sample())
    }
}

impl ModularModule for Oscillator {
    fn inputs(&self) -> &[&str] {
        &["frequency", "fm", "am"]
    }

    fn outputs(&self) -> &[&str] {
        &["audio"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "frequency" => {
                self.set_frequency(value);
                Ok(())
            }
            "fm" => Ok(()), // FM handled during output generation
            "am" => Ok(()), // AM handled during output generation
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&mut self, port: &str) -> Result<f32, String> {
        match port {
            "audio" => Ok(self.generate_sample()),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn reset_inputs(&mut self) {
        // Don't reset frequency - it should be stable
        // Modulation inputs are handled differently (they need to be passed per-sample)
    }
}
