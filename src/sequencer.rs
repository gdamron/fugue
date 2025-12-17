use crate::module::{Module, Processor};
use crate::scale::{Note, Scale};
use crate::signal::{ClockSignal, FrequencySignal, GateSignal};
use crate::synthesis::OscillatorType;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MelodyParams {
    pub allowed_degrees: Arc<Mutex<Vec<usize>>>,
    pub note_weights: Arc<Mutex<Vec<f32>>>,
    pub note_duration: Arc<Mutex<f32>>,
    pub oscillator_type: Arc<Mutex<OscillatorType>>,
}

impl MelodyParams {
    pub fn new(allowed_degrees: Vec<usize>) -> Self {
        let weights = vec![1.0; allowed_degrees.len()];
        Self {
            allowed_degrees: Arc::new(Mutex::new(allowed_degrees)),
            note_weights: Arc::new(Mutex::new(weights)),
            note_duration: Arc::new(Mutex::new(1.0)), // Quarter note (1 beat)
            oscillator_type: Arc::new(Mutex::new(OscillatorType::Sine)),
        }
    }

    pub fn set_allowed_degrees(&self, degrees: Vec<usize>) {
        let mut allowed = self.allowed_degrees.lock().unwrap();
        *allowed = degrees.clone();

        let mut weights = self.note_weights.lock().unwrap();
        weights.resize(degrees.len(), 1.0);
    }

    pub fn set_note_weights(&self, weights: Vec<f32>) {
        *self.note_weights.lock().unwrap() = weights;
    }

    pub fn set_note_duration(&self, duration: f32) {
        *self.note_duration.lock().unwrap() = duration;
    }

    pub fn set_oscillator_type(&self, osc_type: OscillatorType) {
        *self.oscillator_type.lock().unwrap() = osc_type;
    }

    pub fn get_oscillator_type(&self) -> OscillatorType {
        *self.oscillator_type.lock().unwrap()
    }
}

/// MelodyGenerator - accepts ClockSignal and outputs gate+frequency per note
pub struct MelodyGenerator {
    scale: Scale,
    params: MelodyParams,
    rng: StdRng,
    current_note: Note,
    samples_since_note: u64,
    sample_rate: u32,
    tempo: crate::time::Tempo,
}

impl MelodyGenerator {
    pub fn new(
        scale: Scale,
        params: MelodyParams,
        sample_rate: u32,
        tempo: crate::time::Tempo,
    ) -> Self {
        let current_note = Note::new(60);
        Self {
            scale,
            params,
            rng: StdRng::from_entropy(),
            current_note,
            samples_since_note: 0,
            sample_rate,
            tempo,
        }
    }

    pub fn next_note(&mut self) -> Note {
        let allowed = self.params.allowed_degrees.lock().unwrap();
        let weights = self.params.note_weights.lock().unwrap();

        if allowed.is_empty() {
            return Note::new(60);
        }

        let total_weight: f32 = weights.iter().sum();
        let mut random_value = self.rng.gen::<f32>() * total_weight;

        for (i, &degree) in allowed.iter().enumerate() {
            let weight = weights.get(i).unwrap_or(&1.0);
            if random_value < *weight {
                return self.scale.get_note(degree);
            }
            random_value -= weight;
        }

        self.scale.get_note(allowed[0])
    }

    pub fn params(&self) -> &MelodyParams {
        &self.params
    }
}

impl Module for MelodyGenerator {
    fn process(&mut self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "MelodyGenerator"
    }
}

/// Output combines gate and frequency information
#[derive(Debug, Clone, Copy)]
pub struct NoteSignal {
    pub gate: GateSignal,
    pub frequency: FrequencySignal,
}

impl Processor<ClockSignal, NoteSignal> for MelodyGenerator {
    fn process_signal(&mut self, _clock: ClockSignal) -> NoteSignal {
        let note_duration = *self.params.note_duration.lock().unwrap();
        let samples_per_beat = self.tempo.samples_per_beat(self.sample_rate);
        let samples_per_note = (samples_per_beat * note_duration as f64) as u64;

        // Check if we need a new note
        if self.samples_since_note >= samples_per_note {
            self.current_note = self.next_note();
            self.samples_since_note = 0;
        }

        // Calculate envelope (simple ASR)
        let envelope = if self.samples_since_note < samples_per_note / 10 {
            self.samples_since_note as f32 / (samples_per_note as f32 / 10.0)
        } else if self.samples_since_note > samples_per_note * 9 / 10 {
            1.0 - ((self.samples_since_note - samples_per_note * 9 / 10) as f32
                / (samples_per_note as f32 / 10.0))
        } else {
            1.0
        };

        self.samples_since_note += 1;

        NoteSignal {
            gate: GateSignal::new(true, envelope),
            frequency: FrequencySignal::from_midi(self.current_note.midi_note),
        }
    }
}
