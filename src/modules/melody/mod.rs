//! Melody generation module.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::music::{Note, Scale};
use crate::traits::ControlMeta;
use crate::Module;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

pub use self::controls::MelodyControls;

mod controls;
mod inputs;
mod outputs;

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
            None => StdRng::from_entropy(),
        };
        let last_seed_version = controls.seed_version();
        Self {
            ctrl: controls,
            rng,
            last_seed_version,
            current_note,
            inputs: inputs::MelodyInputs::new(),
            last_gate: 0.0,
            outputs: outputs::MelodyOutputs::new(current_note.frequency()),
        }
    }

    /// Selects the next note using weighted random choice.
    ///
    /// Returns middle C (MIDI 60) if no degrees are allowed.
    pub fn next_note(&mut self) -> Note {
        let allowed = self.ctrl.allowed_degrees.lock().unwrap();
        let weights = self.ctrl.note_weights.lock().unwrap();
        let scale = Scale::new(Note::new(self.ctrl.root_note()));

        if allowed.is_empty() {
            return Note::new(60);
        }

        let total_weight: f32 = weights.iter().sum();
        let mut random_value = self.rng.gen::<f32>() * total_weight;

        for (i, &degree) in allowed.iter().enumerate() {
            let weight = weights.get(i).unwrap_or(&1.0);
            if random_value < *weight {
                return scale.get_note(degree);
            }
            random_value -= weight;
        }

        scale.get_note(allowed[0])
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
mod tests {
    use super::*;

    fn make_melody() -> MelodyGenerator {
        let controls = MelodyControls::new(60, vec![0, 2, 3, 5, 7, 9, 10]);
        MelodyGenerator::new(controls)
    }

    #[test]
    fn test_melody_controls_metadata() {
        let melody = make_melody();
        let controls = Module::controls(&melody);

        // root_note + degree_count + 7 degrees + 7 weights = 16
        // (the seed control lives on the ControlSurface, not this legacy list)
        assert_eq!(controls.len(), 16);

        let keys: Vec<&str> = controls.iter().map(|c| c.key.as_str()).collect();
        assert!(keys.contains(&"root_note"));
        assert!(keys.contains(&"degree_count"));
        assert!(keys.contains(&"degree.0"));
        assert!(keys.contains(&"degree.6"));
        assert!(keys.contains(&"note_weight.0"));
        assert!(keys.contains(&"note_weight.6"));
    }

    #[test]
    fn test_melody_get_degree_controls() {
        let melody = make_melody();

        assert_eq!(melody.get_control("degree_count").unwrap(), 7.0);
        assert_eq!(melody.get_control("degree.0").unwrap(), 0.0);
        assert_eq!(melody.get_control("degree.3").unwrap(), 5.0);
        assert_eq!(melody.get_control("note_weight.0").unwrap(), 1.0);
    }

    #[test]
    fn test_melody_set_degree_controls() {
        let mut melody = make_melody();

        // Set a specific degree
        melody.set_control("degree.2", 5.0).unwrap();
        assert_eq!(melody.get_control("degree.2").unwrap(), 5.0);

        // Set a weight
        melody.set_control("note_weight.1", 3.5).unwrap();
        assert_eq!(melody.get_control("note_weight.1").unwrap(), 3.5);
    }

    #[test]
    fn test_melody_degree_count_grow() {
        let mut melody = make_melody();

        melody.set_control("degree_count", 9.0).unwrap();
        assert_eq!(melody.get_control("degree_count").unwrap(), 9.0);

        // New degrees should be sequential after last existing degree (10)
        assert_eq!(melody.get_control("degree.7").unwrap(), 11.0);
        assert_eq!(melody.get_control("degree.8").unwrap(), 12.0);

        // New weights default to 1.0
        assert_eq!(melody.get_control("note_weight.7").unwrap(), 1.0);
        assert_eq!(melody.get_control("note_weight.8").unwrap(), 1.0);

        // Controls metadata should reflect new count
        let controls = Module::controls(&melody);
        // root_note + degree_count + 9 degrees + 9 weights = 20
        assert_eq!(controls.len(), 20);
    }

    #[test]
    fn test_melody_degree_count_shrink() {
        let mut melody = make_melody();

        melody.set_control("degree_count", 3.0).unwrap();
        assert_eq!(melody.get_control("degree_count").unwrap(), 3.0);

        // Accessing beyond the new count should error
        assert!(melody.get_control("degree.3").is_err());
        assert!(melody.get_control("note_weight.3").is_err());
    }

    #[test]
    fn test_melody_out_of_range_degree_errors() {
        let melody = make_melody();

        assert!(melody.get_control("degree.7").is_err());
        assert!(melody.get_control("note_weight.7").is_err());
    }

    #[test]
    fn test_melody_unknown_control_errors() {
        let melody = make_melody();

        assert!(melody.get_control("unknown").is_err());
    }

    #[test]
    fn test_melody_negative_degrees() {
        let controls = MelodyControls::new(60, vec![-2, 0, 2, 5]);
        let melody = MelodyGenerator::new(controls);

        assert_eq!(melody.get_control("degree.0").unwrap(), -2.0);
        assert_eq!(melody.get_control("degree.1").unwrap(), 0.0);
        assert_eq!(melody.get_control("degree.2").unwrap(), 2.0);
        assert_eq!(melody.get_control("degree.3").unwrap(), 5.0);
    }

    #[test]
    fn test_melody_set_negative_degree() {
        let mut melody = make_melody();

        melody.set_control("degree.0", -3.0).unwrap();
        assert_eq!(melody.get_control("degree.0").unwrap(), -3.0);
    }

    /// Drives one gate pulse and returns the resulting frequency.
    fn pulse_note(melody: &mut MelodyGenerator) -> f32 {
        melody.set_input("gate", 1.0).unwrap();
        melody.process(1);
        melody.set_input("gate", 0.0).unwrap();
        melody.process(1);
        melody.get_output("frequency").unwrap()
    }

    fn notes(melody: &mut MelodyGenerator, count: usize) -> Vec<f32> {
        (0..count).map(|_| pulse_note(melody)).collect()
    }

    fn seeded_melody(seed: u64) -> MelodyGenerator {
        let controls = MelodyControls::new(60, vec![0, 2, 3, 5, 7, 9, 10]);
        controls.set_seed(seed);
        MelodyGenerator::new(controls)
    }

    #[test]
    fn test_same_seed_produces_identical_melodies() {
        let a = notes(&mut seeded_melody(42), 32);
        let b = notes(&mut seeded_melody(42), 32);
        assert_eq!(a, b, "same seed must reproduce the same note stream");

        let c = notes(&mut seeded_melody(43), 32);
        assert_ne!(a, c, "different seeds should diverge");
    }

    #[test]
    fn test_reseeding_restarts_the_stream() {
        let mut melody = seeded_melody(7);
        let first = notes(&mut melody, 8);

        // Setting the same seed again restarts the stream from the top.
        melody.controls().set_seed(7);
        let replay = notes(&mut melody, 8);
        assert_eq!(first, replay);
    }

    #[test]
    fn test_seed_control_surface_round_trip() {
        use crate::{ControlSurface, ControlValue};
        let controls = MelodyControls::new(60, vec![0, 2, 4]);
        assert_eq!(controls.seed(), None, "unseeded by default");

        controls
            .set_control("seed", ControlValue::Number(1234.0))
            .unwrap();
        assert_eq!(controls.seed(), Some(1234));
        assert_eq!(
            controls.get_control("seed").unwrap(),
            ControlValue::Number(1234.0)
        );
        assert!(controls.controls().iter().any(|meta| meta.key == "seed"));
    }

    #[test]
    fn test_factory_seed_config() {
        use crate::factory::ModuleFactory;
        let factory = MelodyFactory;
        let build = |seed: u64| {
            let config = serde_json::json!({
                "root_note": 60,
                "scale_degrees": [0, 2, 4, 5, 7],
                "seed": seed
            });
            factory.build(48_000, &config).unwrap()
        };
        let mut first = build(99);
        let mut second = build(99);
        let a: Vec<f32> = (0..16)
            .map(|_| {
                first.module.module_mut().set_input("gate", 1.0).unwrap();
                first.module.module_mut().process(1);
                first.module.module_mut().set_input("gate", 0.0).unwrap();
                first.module.module_mut().process(1);
                first.module.module().get_output("frequency").unwrap()
            })
            .collect();
        let b: Vec<f32> = (0..16)
            .map(|_| {
                second.module.module_mut().set_input("gate", 1.0).unwrap();
                second.module.module_mut().process(1);
                second.module.module_mut().set_input("gate", 0.0).unwrap();
                second.module.module_mut().process(1);
                second.module.module().get_output("frequency").unwrap()
            })
            .collect();
        assert_eq!(a, b, "config seed flows through the factory");
    }
}
