//! Melody generation module.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::music::{Note, Scale};
use crate::traits::ControlMeta;
use crate::Module;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

pub use self::controls::MelodyControls;

mod controls;
mod inputs;
mod outputs;
mod snapshot;

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
        let root_note = config
            .get("root_note")
            .and_then(|v| v.as_u64())
            .unwrap_or(60) as u8;

        let degrees = config
            .get("scale_degrees")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_i64().map(|n| n as i32))
                    .collect()
            })
            .unwrap_or_else(|| vec![0, 2, 4, 5, 7, 9, 11]);

        let controls = MelodyControls::new(root_note, degrees);
        if let Some(seed) = config.get("seed").and_then(|v| v.as_u64()) {
            controls.set_seed(seed);
        }

        if let Some(weights) = config.get("note_weights").and_then(|v| v.as_array()) {
            let weights: Vec<f32> = weights
                .iter()
                .filter_map(|v| v.as_f64().map(|n| n as f32))
                .collect();
            controls.set_note_weights(weights);
        }

        let melody = MelodyGenerator::new(controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(melody)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
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
///
/// # Controls
/// - `degree_count`: Number of active scale degrees (1-128)
/// - `degree.{n}`: Scale degree index at position n (0-127)
/// - `note_weight.{n}`: Probability weight for degree n (0.0-10.0)
pub struct MelodyGenerator {
    ctrl: MelodyControls,
    rng: StdRng,
    /// Seed version last applied to `rng`; re-seeds when the control changes.
    last_seed_version: u64,
    /// Audio-thread copy of the degree table, refreshed without blocking.
    degrees: snapshot::DegreeSnapshot,
    current_note: Note,
    // Modular inputs
    inputs: inputs::MelodyInputs,
    last_gate: f32,
    // Cached outputs (computed in process())
    outputs: outputs::MelodyOutputs,
}

impl MelodyGenerator {
    /// Creates a new melody generator.
    ///
    /// Notes are selected from the given scale according to the controls.
    /// Note changes are triggered by the rising edge of the `gate` input.
    pub fn new(controls: MelodyControls) -> Self {
        let current_note = Note::new(60);
        // A configured seed makes the generator fully deterministic; without
        // one the historical entropy-seeded behavior is preserved.
        let rng = match controls.seed() {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => rand::make_rng(),
        };
        let last_seed_version = controls.seed_version();
        let degrees = snapshot::DegreeSnapshot::new(&controls);
        Self {
            ctrl: controls,
            rng,
            last_seed_version,
            degrees,
            current_note,
            inputs: inputs::MelodyInputs::new(),
            last_gate: 0.0,
            outputs: outputs::MelodyOutputs::new(current_note.frequency()),
        }
    }

    /// Selects the next note using weighted random choice.
    ///
    /// Returns middle C (MIDI 60) if no degrees are allowed.
    ///
    /// Runs on the audio thread: it never blocks or allocates. Degree and
    /// weight edits are picked up when the control table is uncontended.
    pub fn next_note(&mut self) -> Note {
        self.degrees.sync(&self.ctrl);

        if self.degrees.is_empty() {
            return Note::new(60);
        }

        let scale = Scale::new(Note::new(self.ctrl.root_note()));
        scale.get_note(self.degrees.choose(self.rng.random::<f32>()))
    }

    /// Returns a reference to the melody controls.
    pub fn controls(&self) -> &MelodyControls {
        &self.ctrl
    }
}

impl Module for MelodyGenerator {
    fn name(&self) -> &str {
        "MelodyGenerator"
    }

    fn process(&mut self, frames: usize) -> bool {
        // Re-seed when the seed control changed (checked once per block;
        // seeding a ChaCha-based StdRng is stack-only, no allocation).
        let seed_version = self.ctrl.seed_version();
        if seed_version != self.last_seed_version {
            if let Some(seed) = self.ctrl.seed() {
                self.rng = StdRng::seed_from_u64(seed);
            }
            self.last_seed_version = seed_version;
        }

        for i in 0..frames {
            // Detect rising edge of gate input
            let gate = self.inputs.gate(i);
            let gate_high = gate > 0.5;
            let was_low = self.last_gate <= 0.5;

            if gate_high && was_low {
                // Rising edge: select a new note
                self.current_note = self.next_note();
            }

            // Cache outputs
            self.outputs.set(i, self.current_note.frequency(), gate);

            // Remember last gate state for edge detection
            self.last_gate = gate;
        }

        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn controls(&self) -> Vec<ControlMeta> {
        let degree_count = self.ctrl.degree_count();

        let mut controls = Vec::with_capacity(3 + degree_count * 2);

        controls.push(
            ControlMeta::new("root_note", "Root MIDI note number")
                .with_range(0.0, 127.0)
                .with_default(self.ctrl.root_note() as f32),
        );
        controls.push(
            ControlMeta::new("degree_count", "Number of active scale degrees")
                .with_range(1.0, 128.0)
                .with_default(7.0),
        );

        for i in 0..degree_count {
            controls.push(
                ControlMeta::new(
                    format!("degree.{}", i),
                    format!("Scale degree at position {}", i),
                )
                .with_range(-127.0, 127.0)
                .with_default(i as f32),
            );
            controls.push(
                ControlMeta::new(
                    format!("note_weight.{}", i),
                    format!("Probability weight for degree {}", i),
                )
                .with_range(0.0, 10.0)
                .with_default(1.0),
            );
        }

        controls
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "root_note" => Ok(self.ctrl.root_note() as f32),
            "degree_count" => Ok(self.ctrl.degree_count() as f32),
            "seed" => Ok(self.ctrl.seed().unwrap_or(0) as f32),
            _ => {
                if let Some(rest) = key.strip_prefix("degree.") {
                    if let Ok(idx) = rest.parse::<usize>() {
                        return self.ctrl.degree(idx).map(|d| d as f32);
                    }
                }
                if let Some(rest) = key.strip_prefix("note_weight.") {
                    if let Ok(idx) = rest.parse::<usize>() {
                        return self.ctrl.note_weight(idx);
                    }
                }
                Err(format!("Unknown control: {}", key))
            }
        }
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        match key {
            "root_note" => {
                self.ctrl.set_root_note(value as u8);
                Ok(())
            }
            "degree_count" => {
                self.ctrl.set_degree_count(value as usize);
                Ok(())
            }
            "seed" => {
                self.ctrl.set_seed(value.max(0.0) as u64);
                Ok(())
            }
            _ => {
                if let Some(rest) = key.strip_prefix("degree.") {
                    if let Ok(idx) = rest.parse::<usize>() {
                        return self.ctrl.set_degree(idx, value as i32);
                    }
                }
                if let Some(rest) = key.strip_prefix("note_weight.") {
                    if let Ok(idx) = rest.parse::<usize>() {
                        return self.ctrl.set_note_weight(idx, value);
                    }
                }
                Err(format!("Unknown control: {}", key))
            }
        }
    }
}

#[cfg(test)]
mod tests;
