//! Melody generation module.

use std::sync::Arc;

use crate::control_request::{
    local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable, Refusal,
    RtValue,
};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{finite_f32, ConfigKey, ConfigReader};
use crate::music::{Note, Scale};
use crate::payload::{Payload, Retired};
use crate::traits::ControlMeta;
use crate::Module;
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};

use self::declared::{
    degree_index, weight_index, Scale as ScalePayload, Weights, DEGREES, DEGREE_COUNT, MAX_DEGREES,
    NOTE_WEIGHTS, ROOT_NOTE, SEED, TABLE,
};
use self::degrees::Degrees;

mod declared;
mod degrees;
mod inputs;
mod outputs;

/// Factory for constructing MelodyGenerator modules from configuration.
///
/// `degrees` and `note_weights` are arrays, or the same arrays as JSON text
/// (the form a written control records). `degree_count`, `degree.N` and
/// `note_weight.N` restore what was written to them: a position past the
/// count stays hidden until it grows.
pub struct MelodyFactory;

pub(crate) const TYPE_ID: &str = "melody";
const ROOT_NOTE_KEY: ConfigKey = ConfigKey::int::<u8>("root_note");
const SEED_KEY: ConfigKey = ConfigKey::seed("seed");
const DEFAULT_SCALE: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

impl ModuleFactory for MelodyFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const {
            &[
                ROOT_NOTE_KEY,
                SEED_KEY,
                // A whole scale sets the count and every degree; whole
                // weights set every weight.
                ConfigKey::json("degrees").replacing(&["degree_count", "degree"]),
                ConfigKey::json("note_weights").replacing(&["note_weight"]),
                // Kept past the active count, so declared as families.
                ConfigKey::json("degree.N"),
                ConfigKey::json("note_weight.N"),
            ]
        }
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        // A note or seed written as a fraction was held truncated.
        let reader = ConfigReader::new(TYPE_ID, config);
        let refused = |refusal: String| format!("{TYPE_ID} config {refusal}");
        let root_note = reader.int::<u8>(&ROOT_NOTE_KEY)?.unwrap_or(60);
        let seed = reader.int::<u64>(&SEED_KEY)?;
        let scale = match reader.get("degrees") {
            Some(value) => {
                declared::list("degrees", value, declared::degree_of).map_err(refused)?
            }
            None => DEFAULT_SCALE.to_vec(),
        };
        let mut table = Degrees::new(&scale);
        let weights = reader.get("note_weights");
        let weights =
            weights.map(|value| declared::list("note_weights", value, declared::weight_of));
        let weights = weights.transpose().map_err(refused)?;
        if let Some(weights) = &weights {
            table.set_weights(weights);
        }
        if let Some(value) = reader.get("degree_count") {
            table.set_count(number("degree_count", value)? as usize);
        }
        // Degrees and weights written to single positions, as a document
        // records them; one past the count stays hidden until it grows.
        for (key, value) in config.as_object().into_iter().flatten() {
            let (prefix, is_weight) = match key {
                key if key.starts_with("degree.") => ("degree.", false),
                key if key.starts_with("note_weight.") => ("note_weight.", true),
                _ => continue,
            };
            let position = key[prefix.len()..]
                .parse::<usize>()
                .map_err(|_| format!("config '{key}': invalid index"))?;
            let value = number(key, value)?;
            match (position < MAX_DEGREES, is_weight) {
                (false, _) => {}
                // Through f32, as a control write takes it.
                (true, false) => {
                    table.set_degree(position, value as i32);
                }
                (true, true) => {
                    table.set_weight(position, value);
                }
            }
        }

        let cells = Arc::new(ControlCells::new(declared::defaults()));
        let melody = MelodyGenerator::with_parts(root_note, seed, table, cells.clone());
        let surface = DeclaredSurface::new(TABLE.clone(), cells);
        surface.show_payload("degrees", declared::shown(&scale));
        surface.show_payload(
            "note_weights",
            declared::shown(&weights.unwrap_or_default()),
        );
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(melody)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A config number written to a control: a number, or a number as text.
fn number(key: &str, value: &serde_json::Value) -> Result<f32, String> {
    let refuse = || format!("config '{key}': expected a number");
    match value {
        serde_json::Value::Number(_) => {
            finite_f32(value).map_err(|refusal| format!("config '{key}' {refusal}"))
        }
        serde_json::Value::String(text) => text
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(refuse),
        _ => Err(refuse()),
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
/// - `root_note`: Root MIDI note (0-127)
/// - `seed`: RNG seed; setting it restarts the stream
/// - `degrees`, `note_weights`: The whole scale and base weights (JSON text)
/// - `degree_count`: Number of active scale degrees (1-128)
/// - `degree.{n}`: Scale degree at position n (±127)
/// - `note_weight.{n}`: Probability weight for degree n (0.0-10.0)
pub struct MelodyGenerator {
    degrees: Degrees,
    root_note: u8,
    rng: StdRng,
    cells: Arc<ControlCells>,
    current_note: Note,
    inputs: inputs::MelodyInputs,
    last_gate: f32,
    outputs: outputs::MelodyOutputs,
}

impl MelodyGenerator {
    /// Creates a melody over `degrees` from `root_note`, seeded from
    /// entropy. Note changes are triggered by the rising edge of the
    /// `gate` input.
    pub fn new(root_note: u8, degrees: Vec<i32>) -> Self {
        let cells = Arc::new(ControlCells::new(declared::defaults()));
        Self::with_parts(root_note, None, Degrees::new(&degrees), cells)
    }

    /// A melody playing `degrees`, holding its controls in `cells`, which
    /// it fills with what it holds. A configured seed makes it fully
    /// deterministic; without one it runs from entropy.
    fn with_parts(
        root_note: u8,
        seed: Option<u64>,
        degrees: Degrees,
        cells: Arc<ControlCells>,
    ) -> Self {
        let rng = match seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => rand::make_rng(),
        };
        let current_note = Note::new(60);
        let melody = Self {
            degrees,
            root_note: root_note.min(127),
            rng,
            cells,
            current_note,
            inputs: inputs::MelodyInputs::new(),
            last_gate: 0.0,
            outputs: outputs::MelodyOutputs::new(current_note.frequency()),
        };
        let cells = &melody.cells;
        cells.publish(ROOT_NOTE, RtValue::F32(f32::from(melody.root_note)));
        cells.publish(SEED, RtValue::F32(seed.unwrap_or(0) as f32));
        melody.publish_table(true);
        melody
    }

    /// Republishes what every position holds, and the count with the
    /// degrees, after a whole scale or whole weights replaced them.
    fn publish_table(&self, degrees: bool) {
        let table = &self.degrees;
        if degrees {
            let count = table.count() as f32;
            self.cells.publish(DEGREE_COUNT, RtValue::F32(count));
        }
        for position in 0..MAX_DEGREES {
            if degrees {
                let degree = table.degree(position) as f32;
                self.cells
                    .publish(degree_index(position), RtValue::F32(degree));
            }
            let weight = table.weight(position);
            self.cells
                .publish(weight_index(position), RtValue::F32(weight));
        }
    }

    /// Selects the next note using weighted random choice.
    ///
    /// Returns middle C (MIDI 60) if no degrees are allowed.
    pub fn next_note(&mut self) -> Note {
        if self.degrees.is_empty() {
            return Note::new(60);
        }
        let scale = Scale::new(Note::new(self.root_note));
        scale.get_note(self.degrees.choose(self.rng.random::<f32>()))
    }
}

impl Module for MelodyGenerator {
    fn name(&self) -> &str {
        "MelodyGenerator"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            // Detect rising edge of gate input
            let gate = self.inputs.gate(i);
            let gate_high = gate > 0.5;
            let was_low = self.last_gate <= 0.5;

            if gate_high && was_low {
                // Rising edge: select a new note
                self.current_note = self.next_note();
            }

            self.outputs.set(i, self.current_note.frequency(), gate);
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

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    /// Every number is truncated toward zero where the melody holds a whole
    /// one, as a scheduler's ramp has always written them. A seed always
    /// restarts the stream, even the seed it holds.
    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        let RtValue::F32(value) = value else {
            return Err(Refusal::Invalid);
        };
        let held = match control {
            ROOT_NOTE => {
                self.root_note = (value as u8).min(127);
                f32::from(self.root_note)
            }
            DEGREE_COUNT => self.degrees.set_count(value as usize) as f32,
            SEED => {
                let seed = value.max(0.0) as u64;
                self.rng = StdRng::seed_from_u64(seed);
                seed as f32
            }
            control => match declared::position(control) {
                Some((false, position)) => self.degrees.set_degree(position, value as i32) as f32,
                Some((true, position)) => self.degrees.set_weight(position, value),
                None => return Err(Refusal::Unsupported),
            },
        };
        Ok(RtValue::F32(held))
    }

    /// Takes a whole scale or whole weights: copied into the table, and the
    /// written list retired off the audio thread.
    #[allow(private_interfaces)]
    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        let applied = match control {
            DEGREES => payload
                .peek::<ScalePayload>()
                .map(|scale| self.degrees.set_scale(&scale.0))
                .is_some(),
            NOTE_WEIGHTS => payload
                .peek::<Weights>()
                .map(|weights| self.degrees.set_weights(&weights.0))
                .is_some(),
            _ => return Err((Refusal::Unsupported, payload)),
        };
        if !applied {
            return Err((Refusal::Invalid, payload));
        }
        self.publish_table(control == DEGREES);
        Ok(payload.into())
    }

    /// A whole scale rewrites the count and every degree, and every weight
    /// not written to its own position; whole weights rewrite every
    /// weight. The count rewrites none: it only decides which play.
    #[allow(private_interfaces)]
    fn for_each_alias(
        &self,
        control: ControlIndex,
        f: &mut dyn FnMut(&ControlTable, &ControlCells, ControlIndex),
    ) {
        let whole_scale = match control {
            DEGREES => true,
            NOTE_WEIGHTS => false,
            _ => return,
        };
        if whole_scale {
            f(&TABLE, &self.cells, DEGREE_COUNT);
        }
        for position in 0..MAX_DEGREES {
            if whole_scale {
                f(&TABLE, &self.cells, degree_index(position));
            }
            if !whole_scale || !self.degrees.weight_written(position) {
                f(&TABLE, &self.cells, weight_index(position));
            }
        }
    }

    fn controls(&self) -> Vec<ControlMeta> {
        local_controls(self)
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        local_get(self, key)
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        local_set(self, key, value)
    }
}

#[cfg(test)]
mod tests;
