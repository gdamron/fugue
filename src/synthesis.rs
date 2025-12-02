use std::f32::consts::PI;
use crate::signal::{AudioSignal, FrequencySignal};
use crate::module::{Module, Generator, Processor};

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
                if self.phase < 0.5 { 1.0 } else { -1.0 }
            }
            OscillatorType::Sawtooth => 2.0 * self.phase - 1.0,
            OscillatorType::Triangle => {
                4.0 * (self.phase - 0.5).abs() - 1.0
            }
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

/// Low-pass filter - processes audio signals
pub struct Filter {
    cutoff: f32,
    resonance: f32,
    prev_output: f32,
    sample_rate: u32,
}

impl Filter {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            cutoff: 1000.0,
            resonance: 0.5,
            prev_output: 0.0,
            sample_rate,
        }
    }
    
    pub fn with_cutoff(mut self, cutoff: f32) -> Self {
        self.cutoff = cutoff;
        self
    }
    
    pub fn with_resonance(mut self, resonance: f32) -> Self {
        self.resonance = resonance.clamp(0.0, 1.0);
        self
    }

    pub fn set_cutoff(&mut self, cutoff: f32) {
        self.cutoff = cutoff;
    }

    pub fn set_resonance(&mut self, resonance: f32) {
        self.resonance = resonance.clamp(0.0, 1.0);
    }
}

impl Module for Filter {
    fn process(&mut self) -> bool {
        true
    }
    
    fn name(&self) -> &str {
        "Filter"
    }
}

impl Processor<AudioSignal, AudioSignal> for Filter {
    fn process_signal(&mut self, input: AudioSignal) -> AudioSignal {
        let alpha = 0.1 + self.resonance * 0.5;
        self.prev_output = alpha * input.value + (1.0 - alpha) * self.prev_output;
        AudioSignal::new(self.prev_output)
    }
}
