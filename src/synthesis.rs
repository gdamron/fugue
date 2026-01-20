use std::sync::{Arc, Mutex};

use crate::module::{Generator, Module, Processor};
use crate::oscillator::{Oscillator, OscillatorType};
use crate::sequencer::NoteSignal;
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

/// Voice - converts NoteSignal (gate + frequency) to AudioSignal
/// Combines an oscillator with envelope following
pub struct Voice {
    oscillator: Oscillator,
    osc_type: Arc<Mutex<OscillatorType>>,
}

impl Voice {
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        Self {
            oscillator: Oscillator::new(sample_rate, osc_type),
            osc_type: Arc::new(Mutex::new(osc_type)),
        }
    }

    pub fn with_osc_type_control(mut self, osc_type: Arc<Mutex<OscillatorType>>) -> Self {
        self.osc_type = osc_type;
        self
    }
}

impl Module for Voice {
    fn process(&mut self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "Voice"
    }
}

impl Processor<NoteSignal, AudioSignal> for Voice {
    fn process_signal(&mut self, input: NoteSignal) -> AudioSignal {
        // Update oscillator type if it changed
        let osc_type = *self.osc_type.lock().unwrap();
        self.oscillator.set_type(osc_type);

        // Update frequency from note
        self.oscillator.set_frequency(input.frequency.hz);

        // Generate audio and apply envelope (velocity)
        // Scale by 0.3 to prevent clipping
        let audio = self.oscillator.output();
        AudioSignal::new(audio.value * input.gate.value * 0.3)
    }
}
