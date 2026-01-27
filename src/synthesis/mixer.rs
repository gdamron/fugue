use crate::module::Module;
use crate::signal::AudioSignal;

/// Combines multiple audio signals into a single output.
///
/// Automatically applies gain adjustment based on the number of inputs
/// to prevent clipping when mixing.
pub struct Mixer {
    gain: f32,
}

impl Mixer {
    /// Creates a new mixer for the specified number of inputs.
    ///
    /// Gain is automatically set to `1/sqrt(num_inputs)` to prevent clipping.
    pub fn new(num_inputs: usize) -> Self {
        Self {
            gain: 1.0 / (num_inputs as f32).sqrt(),
        }
    }

    /// Overrides the automatic gain with a custom value.
    pub fn with_gain(mut self, gain: f32) -> Self {
        self.gain = gain;
        self
    }

    /// Mixes all input signals and returns the combined output.
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
