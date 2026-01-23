use std::sync::{Arc, Mutex};

use crate::module::{Generator, Module, Processor};
use crate::oscillator::{Oscillator, OscillatorType};
use crate::sequencer::NoteSignal;
use crate::signal::AudioSignal;

/// Converts note information into audio output.
///
/// A voice combines an oscillator with envelope following, processing
/// [`NoteSignal`] input (gate + frequency) into audio samples.
pub struct Voice {
    oscillator: Oscillator,
    osc_type: Arc<Mutex<OscillatorType>>,
}

impl Voice {
    /// Creates a new voice with the given sample rate and oscillator type.
    pub fn new(sample_rate: u32, osc_type: OscillatorType) -> Self {
        Self {
            oscillator: Oscillator::new(sample_rate, osc_type),
            osc_type: Arc::new(Mutex::new(osc_type)),
        }
    }

    /// Sets a shared oscillator type control for real-time waveform changes.
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
        let osc_type = *self.osc_type.lock().unwrap();
        self.oscillator.set_type(osc_type);
        self.oscillator.set_frequency(input.frequency.hz);

        let audio = self.oscillator.output();
        AudioSignal::new(audio.value * input.gate.value * 0.3)
    }
}
