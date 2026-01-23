use crate::module::{Generator, Module};
use crate::signal::AudioSignal;

use super::{Oscillator, OscillatorType};

/// Input values for frequency and amplitude modulation.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModulationInputs {
    /// Frequency modulation input signal value.
    pub frequency: f32,
    /// Amplitude modulation input signal value.
    pub amplitude: f32,
}

/// An oscillator that accepts external modulation signals for FM/AM synthesis.
///
/// Wraps a standard [`Oscillator`] and provides methods to process
/// modulation inputs for more complex sound design.
pub struct ModulatedOscillator {
    oscillator: Oscillator,
    base_frequency: f32,
}

impl ModulatedOscillator {
    /// Creates a new modulated oscillator with the given sample rate and waveform type.
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        Self {
            oscillator: Oscillator::new(sample_rate, osc_type),
            base_frequency: 440.0,
        }
    }

    /// Sets the base frequency in Hz.
    pub fn with_frequency(mut self, freq: f32) -> Self {
        self.base_frequency = freq;
        self.oscillator.set_frequency(freq);
        self
    }

    /// Sets the frequency modulation depth in Hz.
    pub fn with_fm_amount(mut self, amount: f32) -> Self {
        self.oscillator.set_fm_amount(amount);
        self
    }

    /// Sets the amplitude modulation depth (0.0 to 1.0).
    pub fn with_am_amount(mut self, amount: f32) -> Self {
        self.oscillator.set_am_amount(amount);
        self
    }

    /// Changes the oscillator waveform type.
    pub fn set_type(&mut self, osc_type: OscillatorType) {
        self.oscillator.set_type(osc_type);
    }

    /// Generates a sample with the given modulation inputs applied.
    pub fn process_with_modulation(&mut self, mod_inputs: ModulationInputs) -> AudioSignal {
        AudioSignal::new(
            self.oscillator
                .generate_sample_with_modulation(mod_inputs.frequency, mod_inputs.amplitude),
        )
    }
}

impl Module for ModulatedOscillator {
    fn process(&mut self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "ModulatedOscillator"
    }
}

impl Generator<AudioSignal> for ModulatedOscillator {
    fn output(&mut self) -> AudioSignal {
        AudioSignal::new(self.oscillator.generate_sample())
    }
}
