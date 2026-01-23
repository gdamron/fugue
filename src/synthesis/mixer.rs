use crate::module::Module;
use crate::signal::AudioSignal;

/// Mixer - combines multiple audio signals into one
/// In Eurorack terms, this is like a mixer module
pub struct Mixer {
    num_inputs: usize,
    gain: f32,
}

impl Mixer {
    pub fn new(num_inputs: usize) -> Self {
        Self {
            num_inputs,
            gain: 1.0 / (num_inputs as f32).sqrt(), // Auto-adjust gain to prevent clipping
        }
    }

    pub fn with_gain(mut self, gain: f32) -> Self {
        self.gain = gain;
        self
    }

    /// Mix multiple audio signals
    pub fn mix(&mut self, inputs: &[AudioSignal]) -> AudioSignal {
        let sum: f32 = inputs.iter().map(|s| s.value).sum();
        AudioSignal::new(sum * self.gain)
    }
}

impl Module for Mixer {
    fn process(&mut self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "Mixer"
    }
}
