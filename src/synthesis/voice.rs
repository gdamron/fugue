use std::sync::{Arc, Mutex};

use crate::module::{Generator, Module, Processor};
use crate::oscillator::{Oscillator, OscillatorType};
use crate::sequencer::NoteSignal;
use crate::signal::AudioSignal;

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
