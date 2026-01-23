use crate::module::{Module, Processor};
use crate::signal::AudioSignal;

/// A simple low-pass filter that attenuates high frequencies.
///
/// Uses a one-pole IIR filter topology. The resonance parameter
/// affects the filter's response character.
#[allow(dead_code)]
pub struct Filter {
    cutoff: f32,
    resonance: f32,
    prev_output: f32,
    sample_rate: u32,
}

impl Filter {
    /// Creates a new filter with the given sample rate.
    ///
    /// Defaults to 1000 Hz cutoff and 0.5 resonance.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            cutoff: 1000.0,
            resonance: 0.5,
            prev_output: 0.0,
            sample_rate,
        }
    }

    /// Sets the cutoff frequency in Hz.
    pub fn with_cutoff(mut self, cutoff: f32) -> Self {
        self.cutoff = cutoff;
        self
    }

    /// Sets the resonance amount (0.0 to 1.0).
    pub fn with_resonance(mut self, resonance: f32) -> Self {
        self.resonance = resonance.clamp(0.0, 1.0);
        self
    }

    /// Sets the cutoff frequency in Hz.
    pub fn set_cutoff(&mut self, cutoff: f32) {
        self.cutoff = cutoff;
    }

    /// Sets the resonance amount (0.0 to 1.0).
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
