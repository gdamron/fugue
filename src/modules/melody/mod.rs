//! Melody generation module.

use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::music::{Mode, Note, Scale};
use crate::Module;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

pub use self::params::MelodyParams;

mod params;

/// Factory for constructing MelodyGenerator modules from configuration.
pub struct MelodyFactory;

impl ModuleFactory for MelodyFactory {
    fn type_id(&self) -> &'static str {
        "melody"
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let root = Note::new(
            config
                .get("root_note")
                .and_then(|v| v.as_u64())
                .unwrap_or(60) as u8,
        );

        let mode = parse_mode(
            config
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("dorian"),
        )?;

        let scale = Scale::new(root, mode);

        let degrees = config
            .get("scale_degrees")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_u64().map(|n| n as usize))
                    .collect()
            })
            .unwrap_or_else(|| vec![0, 1, 2, 3, 4, 5, 6]);

        let params = MelodyParams::new(degrees);

        if let Some(weights) = config.get("note_weights").and_then(|v| v.as_array()) {
            let weights: Vec<f32> = weights
                .iter()
                .filter_map(|v| v.as_f64().map(|n| n as f32))
                .collect();
            params.set_note_weights(weights);
        }

        let melody = MelodyGenerator::new(scale, params.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(melody)),
            handles: vec![(
                "params".to_string(),
                Arc::new(params) as Arc<dyn Any + Send + Sync>,
            )],
        })
    }
}

/// Parses a mode string into a Mode enum.
fn parse_mode(mode_str: &str) -> Result<Mode, Box<dyn std::error::Error>> {
    match mode_str.to_lowercase().as_str() {
        "ionian" | "major" => Ok(Mode::Ionian),
        "dorian" => Ok(Mode::Dorian),
        "phrygian" => Ok(Mode::Phrygian),
        "lydian" => Ok(Mode::Lydian),
        "mixolydian" => Ok(Mode::Mixolydian),
        "aeolian" | "minor" => Ok(Mode::Aeolian),
        "locrian" => Ok(Mode::Locrian),
        _ => Err(format!("Unknown mode: {}", mode_str).into()),
    }
}

/// Generates melodies by selecting notes from a scale based on weighted probabilities.
///
/// Receives a `gate` input signal from the clock. On each rising edge of the gate,
/// a new note is selected from the scale. The gate signal is passed through to
/// the output, allowing downstream ADSR envelopes to shape the note.
///
/// # Inputs
/// - `gate`: Gate signal from clock (rising edge triggers new note selection)
///
/// # Outputs
/// - `frequency`: Current note frequency in Hz
/// - `gate`: Pass-through of the input gate signal
pub struct MelodyGenerator {
    scale: Scale,
    params: MelodyParams,
    rng: StdRng,
    current_note: Note,
    // Modular inputs
    gate_in: f32,
    last_gate: f32,
    // Cached outputs (computed in process())
    cached_frequency: f32,
    cached_gate: f32,
    last_processed_sample: u64, // For pull-based processing
}

impl MelodyGenerator {
    /// Creates a new melody generator.
    ///
    /// Notes are selected from the given scale according to the parameters.
    /// Note changes are triggered by the rising edge of the `gate` input.
    pub fn new(scale: Scale, params: MelodyParams) -> Self {
        let current_note = Note::new(60);
        Self {
            scale,
            params,
            rng: StdRng::from_entropy(),
            current_note,
            gate_in: 0.0,
            last_gate: 0.0,
            cached_frequency: current_note.frequency(),
            cached_gate: 0.0,
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
    fn name(&self) -> &str {
        "MelodyGenerator"
    }

    fn process(&mut self) -> bool {
        // Detect rising edge of gate input
        let gate_high = self.gate_in > 0.5;
        let was_low = self.last_gate <= 0.5;

        if gate_high && was_low {
            // Rising edge: select a new note
            self.current_note = self.next_note();
        }

        // Cache outputs
        self.cached_frequency = self.current_note.frequency();
        self.cached_gate = self.gate_in; // Pass through gate signal

        // Remember last gate state for edge detection
        self.last_gate = self.gate_in;

        true
    }

    fn inputs(&self) -> &[&str] {
        &["gate"]
    }

    fn outputs(&self) -> &[&str] {
        &["frequency", "gate"]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        match port {
            "gate" => {
                self.gate_in = value;
                Ok(())
            }
            _ => Err(format!("Unknown input port: {}", port)),
        }
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        match port {
            "frequency" => Ok(self.cached_frequency),
            "gate" => Ok(self.cached_gate),
            _ => Err(format!("Unknown output port: {}", port)),
        }
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }
}
