use crate::module::{Module, Processor};
use crate::scale::{Note, Scale};
use crate::signal::{Audio, ClockSignal, FrequencySignal};
use crate::time::Tempo;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::{MelodyParams, NoteSignal};

/// MelodyGenerator - accepts ClockSignal and outputs gate+frequency per note
pub struct MelodyGenerator {
    scale: Scale,
    params: MelodyParams,
    rng: StdRng,
    current_note: Note,
    samples_since_note: u64,
    sample_rate: u32,
    tempo: Tempo,
}

impl MelodyGenerator {
    pub fn new(scale: Scale, params: MelodyParams, sample_rate: u32, tempo: Tempo) -> Self {
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
            gate: Audio::gate(true, envelope),
            frequency: FrequencySignal::from_midi(self.current_note.midi_note),
        }
    }
}
