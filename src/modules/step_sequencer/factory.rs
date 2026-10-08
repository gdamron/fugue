use super::*;
use crate::module_config::{ConfigKey, ConfigReader};

/// Factory for constructing StepSequencer modules from configuration.
///
/// # Configuration Options
///
/// - `base_note` (u8): Base MIDI note added to step values (default: 48, C3)
/// - `steps` (usize): Number of steps in pattern (default: 16)
/// - `gate_length` (f32): Default gate length ratio 0.0-1.0 (default: 0.5)
/// - `mode` (string): `"loop"` (default) or `"one_shot"` (play once, fire `end`)
/// - `pattern` (array): Array of step objects
///
/// # Step Object Format
///
/// ```json
/// { "note": 0, "gate": 0.8 }  // Note with custom gate length
/// { "note": 7 }               // Note with default gate length
/// { "note": null }            // Rest (no note)
/// ```
///
/// # Example
///
/// ```json
/// {
///   "id": "bass_seq",
///   "type": "step_sequencer",
///   "config": {
///     "base_note": 36,
///     "steps": 16,
///     "gate_length": 0.5,
///     "pattern": [
///       { "note": 0, "gate": 0.8 },
///       { "note": null },
///       { "note": 7 },
///       { "note": 5 }
///     ]
///   }
/// }
/// ```
pub struct StepSequencerFactory;

const TYPE_ID: &str = "step_sequencer";
const BASE_NOTE: ConfigKey = ConfigKey::int::<u8>("base_note");
const STEPS: ConfigKey = ConfigKey::int::<usize>("steps");
const GATE_LENGTH: ConfigKey = ConfigKey::float("gate_length");
const GRACE_DURATION_MS: ConfigKey = ConfigKey::float("grace_duration_ms");

impl ModuleFactory for StepSequencerFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const {
            &[
                BASE_NOTE,
                STEPS,
                GATE_LENGTH,
                GRACE_DURATION_MS,
                ConfigKey::text("pattern_json"),
                ConfigKey::json("pattern"),
                ConfigKey::text("mode"),
                ConfigKey::text("grace_placement"),
            ]
        }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let base_note = reader.int::<u8>(&BASE_NOTE)?.unwrap_or(DEFAULT_BASE_NOTE);
        let steps = reader.int::<usize>(&STEPS)?.unwrap_or(DEFAULT_STEPS);
        let gate_length = reader.float(&GATE_LENGTH)?.unwrap_or(DEFAULT_GATE_LENGTH);

        // `pattern_json` is the pattern control's key, which an authored
        // write records; it wins over `pattern`.
        let pattern = match config.get("pattern_json").and_then(|value| value.as_str()) {
            Some(json) => parse_pattern(Some(&serde_json::from_str(json)?), "pattern_json"),
            None => parse_pattern(config.get("pattern"), "pattern"),
        }
        .map_err(|error| error.refused_by(&reader))?;

        let controls = StepSequencerControls::new_with_values(base_note, steps, gate_length);
        if let Some(mode) = config.get("mode").and_then(|v| v.as_str()) {
            controls.set_mode(mode)?;
        }
        if let Some(ms) = reader.float(&GRACE_DURATION_MS)? {
            controls.set_grace_duration_ms(ms);
        }
        if let Some(placement) = config.get("grace_placement").and_then(|v| v.as_str()) {
            controls.set_grace_placement(placement)?;
        }

        let seq =
            StepSequencer::new_with_controls(sample_rate, controls.clone()).with_pattern(pattern);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(seq)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}
