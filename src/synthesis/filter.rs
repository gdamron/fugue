use crate::module::{Module, Processor};
use crate::signal::AudioSignal;

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
