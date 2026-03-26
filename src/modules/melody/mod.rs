//! Melody generation module.

use std::any::Any;
use std::sync::{Arc, Mutex};

use crate::factory::{ModuleBuildResult, ModuleFactory};
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
        let root = Note::new(
            config
                .get("root_note")
                .and_then(|v| v.as_u64())
                .unwrap_or(60) as u8,
        );

        let scale = Scale::new(root);

        let degrees = config
            .get("scale_degrees")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_i64().map(|n| n as i32))
                    .collect()
            })
            .unwrap_or_else(|| vec![0, 2, 4, 5, 7, 9, 11]);

        let controls = MelodyControls::new(degrees);

        if let Some(weights) = config.get("note_weights").and_then(|v| v.as_array()) {
            let weights: Vec<f32> = weights
                .iter()
                .filter_map(|v| v.as_f64().map(|n| n as f32))
                .collect();
            controls.set_note_weights(weights);
        }

        let melody = MelodyGenerator::new(scale, controls.clone());

        Ok(ModuleBuildResult {
            module: Arc::new(Mutex::new(melody)),
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
    scale: Scale,
    ctrl: MelodyControls,
    rng: StdRng,
    current_note: Note,
    // Modular inputs
    inputs: inputs::MelodyInputs,
    last_gate: f32,
    // Cached outputs (computed in process())
    outputs: outputs::MelodyOutputs,
    last_processed_sample: u64, // For pull-based processing
}

impl MelodyGenerator {
    /// Creates a new melody generator.
    ///
    /// Notes are selected from the given scale according to the controls.
    /// Note changes are triggered by the rising edge of the `gate` input.
    pub fn new(scale: Scale, controls: MelodyControls) -> Self {
        let current_note = Note::new(60);
        Self {
            scale,
            ctrl: controls,
            rng: StdRng::from_entropy(),
            current_note,
            inputs: inputs::MelodyInputs::new(),
            last_gate: 0.0,
            outputs: outputs::MelodyOutputs::new(current_note.frequency()),
            last_processed_sample: 0,
        }
    }

    /// Selects the next note using weighted random choice.
    ///
    /// Returns middle C (MIDI 60) if no degrees are allowed.
    pub fn next_note(&mut self) -> Note {
        let allowed = self.ctrl.allowed_degrees.lock().unwrap();
        let weights = self.ctrl.note_weights.lock().unwrap();

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

    /// Returns a reference to the melody controls.
    pub fn controls(&self) -> &MelodyControls {
        &self.ctrl
    }
}

impl Module for MelodyGenerator {
    fn name(&self) -> &str {
        "MelodyGenerator"
    }

    fn process(&mut self) -> bool {
        // Detect rising edge of gate input
        let gate_high = self.inputs.gate() > 0.5;
        let was_low = self.last_gate <= 0.5;

        if gate_high && was_low {
            // Rising edge: select a new note
            self.current_note = self.next_note();
        }

        // Cache outputs
        self.outputs
            .set(self.current_note.frequency(), self.inputs.gate());

        // Remember last gate state for edge detection
        self.last_gate = self.inputs.gate();

        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn controls(&self) -> Vec<ControlMeta> {
        let degree_count = self.ctrl.degree_count();

        let mut controls = Vec::with_capacity(2 + degree_count * 2);

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
            "degree_count" => Ok(self.ctrl.degree_count() as f32),
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
            "degree_count" => {
                self.ctrl.set_degree_count(value as usize);
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
        let scale = Scale::new(Note::new(60));
        let controls = MelodyControls::new(vec![0, 2, 3, 5, 7, 9, 10]);
        MelodyGenerator::new(scale, controls)
    }

    #[test]
    fn test_melody_controls_metadata() {
        let melody = make_melody();
        let controls = Module::controls(&melody);

        // degree_count + 7 degrees + 7 weights = 15
        assert_eq!(controls.len(), 15);

        let keys: Vec<&str> = controls.iter().map(|c| c.key.as_str()).collect();
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
        // degree_count + 9 degrees + 9 weights = 19
        assert_eq!(controls.len(), 19);
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
        let scale = Scale::new(Note::new(60));
        let controls = MelodyControls::new(vec![-2, 0, 2, 5]);
        let melody = MelodyGenerator::new(scale, controls);

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
}
