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
pub struct Oscillator {
    osc_type: OscillatorType,
    frequency: f32,
    phase: f32,
    sample_rate: u32,
}

impl Oscillator {
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        Self {
            osc_type,
            frequency: 440.0,
            phase: 0.0,
            sample_rate,
        }
    }

    pub fn with_frequency(mut self, freq: f32) -> Self {
        self.frequency = freq;
        self
    }

    pub fn set_frequency(&mut self, freq: f32) {
        self.frequency = freq;
    }

    pub fn set_type(&mut self, osc_type: OscillatorType) {
        self.osc_type = osc_type;
    }

    fn generate_sample(&mut self) -> f32 {
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

        self.phase += self.frequency / self.sample_rate as f32;
        self.phase %= 1.0;

        sample
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
