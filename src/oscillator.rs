use crate::{
    module::{Generator, Module, Processor},
    AudioSignal, FrequencySignal,
};
use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OscillatorType {
    Sine,
    Square,
    Sawtooth,
    Triangle,
}

/// Oscillator - can work as either a Generator (with fixed frequency)
/// or a Processor (accepting FrequencySignal)
/// Now supports FM (Frequency Modulation) and AM (Amplitude Modulation)
pub struct Oscillator {
    osc_type: OscillatorType,
    frequency: f32,
    phase: f32,
    sample_rate: u32,
    // Modulation parameters
    fm_amount: f32, // Frequency modulation depth (in Hz)
    am_amount: f32, // Amplitude modulation depth (0.0 to 1.0)
}

impl Oscillator {
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

    pub fn with_frequency(mut self, freq: f32) -> Self {
        self.frequency = freq;
        self
    }

    pub fn with_fm_amount(mut self, amount: f32) -> Self {
        self.fm_amount = amount;
        self
    }

    pub fn with_am_amount(mut self, amount: f32) -> Self {
        self.am_amount = amount.clamp(0.0, 1.0);
        self
    }

    pub fn set_frequency(&mut self, freq: f32) {
        self.frequency = freq;
    }

    pub fn set_type(&mut self, osc_type: OscillatorType) {
        self.osc_type = osc_type;
    }

    pub fn set_fm_amount(&mut self, amount: f32) {
        self.fm_amount = amount;
    }

    pub fn set_am_amount(&mut self, amount: f32) {
        self.am_amount = amount.clamp(0.0, 1.0);
    }

    /// Generate a sample with optional frequency and amplitude modulation
    pub fn generate_sample_with_modulation(&mut self, freq_mod: f32, amp_mod: f32) -> f32 {
        // Apply frequency modulation
        let modulated_freq = self.frequency + (freq_mod * self.fm_amount);

        // Generate waveform
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

        // Advance phase with modulated frequency
        self.phase += modulated_freq / self.sample_rate as f32;
        self.phase %= 1.0;

        // Apply amplitude modulation
        // amp_mod is expected to be in range [-1, 1]
        // We map it to [0, 1] for amplitude scaling
        let amp_scale = if self.am_amount > 0.0 {
            let normalized_amp = (amp_mod + 1.0) * 0.5; // Convert [-1, 1] to [0, 1]
            1.0 - self.am_amount + (normalized_amp * self.am_amount)
        } else {
            1.0
        };

        sample * amp_scale
    }

    fn generate_sample(&mut self) -> f32 {
        // No modulation version
        self.generate_sample_with_modulation(0.0, 0.0)
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    // Legacy API for backward compatibility
    pub fn next_sample(&mut self) -> f32 {
        self.generate_sample()
    }
}

// Oscillator as a Generator (fixed frequency)
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

// Oscillator as a Processor (accepts FrequencySignal)
impl Processor<FrequencySignal, AudioSignal> for Oscillator {
    fn process_signal(&mut self, input: FrequencySignal) -> AudioSignal {
        self.set_frequency(input.hz);
        AudioSignal::new(self.generate_sample())
    }
}

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
        self.oscillator = self.oscillator.with_fm_amount(amount);
        self
    }

    pub fn with_am_amount(mut self, amount: f32) -> Self {
        self.oscillator = self.oscillator.with_am_amount(amount);
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
