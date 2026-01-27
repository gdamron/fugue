use crate::module::{ModularModule, Module, Processor};
use crate::scale::{Note, Scale};
use crate::signal::{Audio, ClockSignal, FrequencySignal};
use crate::time::Tempo;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::{MelodyParams, NoteSignal};

/// Generates melodies by selecting notes from a scale based on weighted probabilities.
///
/// Processes [`ClockSignal`] input and outputs [`NoteSignal`] with gate and
/// frequency information. Note selection uses weighted random choice from
/// the allowed scale degrees.
///
/// The gate output is a brief trigger pulse (1ms) at the start of each note,
/// followed by immediate release. This allows downstream ADSR envelopes to
/// control the full note duration and shape.
pub struct MelodyGenerator {
    scale: Scale,
    params: MelodyParams,
    rng: StdRng,
    current_note: Note,
    samples_since_note: u64,
    sample_rate: u32,
    tempo: Tempo,
    // Modular inputs
    beat_in: f32,
    // Cached outputs (computed in process())
    cached_frequency: f32,
    cached_gate: f32,
    cached_trigger: f32,
    last_processed_sample: u64, // For pull-based processing
}

impl MelodyGenerator {
    /// Creates a new melody generator.
    ///
    /// Notes are selected from the given scale according to the parameters.
    /// The tempo controls note timing.
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
            beat_in: 0.0,
            cached_frequency: current_note.frequency(),
            cached_gate: 0.0,
            cached_trigger: 0.0,
            last_processed_sample: 0,
        }
    }

    /// Selects the next note using weighted random choice.
    ///
    /// Returns middle C (MIDI 60) if no degrees are allowed.
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

    /// Returns a reference to the melody parameters.
    pub fn params(&self) -> &MelodyParams {
        &self.params
    }
}

impl Module for MelodyGenerator {
    fn process(&mut self) -> bool {
        // Compute note timing
        let note_duration = *self.params.note_duration.lock().unwrap();
        let samples_per_beat = self.tempo.samples_per_beat(self.sample_rate);
        let samples_per_note = (samples_per_beat * note_duration as f64) as u64;

        // Check if it's time for a new note
        if self.samples_since_note >= samples_per_note {
            self.current_note = self.next_note();
            self.samples_since_note = 0;
        }

        // Compute outputs
        // Gate: HIGH for entire note duration (for ADSR envelope)
        let gate_on = true; // Always high during the note
                            // Trigger: brief pulse at start of note (for triggering other modules)
        let is_trigger = self.samples_since_note == 0;

        // Cache all outputs (BEFORE incrementing sample counter!)
        self.cached_frequency = self.current_note.frequency();
        self.cached_gate = if gate_on { 1.0 } else { 0.0 };
        self.cached_trigger = if is_trigger { 1.0 } else { 0.0 };

        // Now increment counter (only once per sample!)
        self.samples_since_note += 1;

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

        // Trigger length: 1ms pulse at the start of each note
        let trigger_samples = (self.sample_rate as f64 / 1000.0).max(1.0) as u64;

        if self.samples_since_note >= samples_per_note {
            self.current_note = self.next_note();
            self.samples_since_note = 0;
        }

        // Gate is high only for the brief trigger pulse at the start
        // This mimics a clock/trigger signal: brief pulse followed by immediate release
        let gate_on = self.samples_since_note < trigger_samples;

        self.samples_since_note += 1;

        NoteSignal {
            gate: Audio::gate(gate_on, 1.0),
            frequency: FrequencySignal::from_midi(self.current_note.midi_note),
        }
    }
}

impl ModularModule for MelodyGenerator {
    fn inputs(&self) -> &[&str] {
        &["beat"]
    }

    fn outputs(&self) -> &[&str] {
        &["frequency", "gate", "trigger"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "beat" => {
                self.beat_in = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&mut self, port: &str) -> Result<f32, String> {
        // Just return cached values - NO state changes!
        match port {
            "frequency" => Ok(self.cached_frequency),
            "gate" => Ok(self.cached_gate),
            "trigger" => Ok(self.cached_trigger),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn reset_inputs(&mut self) {
        self.beat_in = 0.0;
        // phase_in removed - not used
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn get_cached_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "frequency" => Ok(self.cached_frequency),
            "gate" => Ok(self.cached_gate),
            "trigger" => Ok(self.cached_trigger),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }
}
