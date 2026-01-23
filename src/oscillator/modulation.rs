use crate::module::{Generator, Module};
use crate::signal::AudioSignal;

use super::{Oscillator, OscillatorType};

/// Modulation inputs for an oscillator
#[derive(Debug, Clone, Copy, Default)]
pub struct ModulationInputs {
    pub frequency: f32, // Frequency modulation input (audio signal value)
    pub amplitude: f32, // Amplitude modulation input (audio signal value)
}

/// ModulatedOscillator - accepts modulation inputs as well as base frequency
/// This is used in the patch system for FM/AM synthesis
pub struct ModulatedOscillator {
    oscillator: Oscillator,
    base_frequency: f32,
}

impl ModulatedOscillator {
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        Self {
            oscillator: Oscillator::new(sample_rate, osc_type),
            base_frequency: 440.0,
        }
    }

    pub fn with_frequency(mut self, freq: f32) -> Self {
        self.base_frequency = freq;
        self.oscillator.set_frequency(freq);
        self
    }

    pub fn with_fm_amount(mut self, amount: f32) -> Self {
        self.oscillator.set_fm_amount(amount);
        self
    }

    pub fn with_am_amount(mut self, amount: f32) -> Self {
        self.oscillator.set_am_amount(amount);
        self
    }

    pub fn set_type(&mut self, osc_type: OscillatorType) {
        self.oscillator.set_type(osc_type);
    }

    /// Process with modulation inputs
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
