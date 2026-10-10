use super::*;
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::payload::Shared;
use crate::ControlSurface;

/// Factory for constructing StepSequencer modules from configuration.
///
/// # Configuration Options
///
/// - `root_note` (u8): Root MIDI note added to step values (default: 48, C3)
/// - `step_count` (usize): Number of steps in pattern (default: 16)
/// - `gate_length` (f32): Default gate length ratio 0.0-1.0 (default: 0.5)
/// - `mode` (string): `"loop"` (default) or `"one_shot"` (play once, fire `ended`)
/// - `grace_duration` (f32): Seconds per grace note (default: 0.06)
/// - `pattern` (array): Array of step objects, or the same array as JSON text
///   (the form a written `pattern` control records)
///
/// # Step Object Format
///
/// ```json
/// { "note": 0, "gate_length": 0.8 }  // Note with custom gate length
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
///     "root_note": 36,
///     "step_count": 16,
///     "gate_length": 0.5,
///     "pattern": [
///       { "note": 0, "gate_length": 0.8 },
///       { "note": null },
///       { "note": 7 },
///       { "note": 5 }
///     ]
///   }
/// }
/// ```
pub struct StepSequencerFactory;

const TYPE_ID: &str = "step_sequencer";
const ROOT_NOTE: ConfigKey = ConfigKey::int::<u8>("root_note");
const STEP_COUNT: ConfigKey = ConfigKey::int::<usize>("step_count");
const GATE_LENGTH: ConfigKey = ConfigKey::float("gate_length");
const GRACE_DURATION: ConfigKey = ConfigKey::float("grace_duration");

impl ModuleFactory for StepSequencerFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const {
            &[
                ROOT_NOTE,
                STEP_COUNT,
                GATE_LENGTH,
                GRACE_DURATION,
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
        let root_note = reader.int::<u8>(&ROOT_NOTE)?.unwrap_or(DEFAULT_ROOT_NOTE);
        let step_count = reader.int::<usize>(&STEP_COUNT)?.unwrap_or(DEFAULT_STEPS);
        let gate_length = reader.float(&GATE_LENGTH)?.unwrap_or(DEFAULT_GATE_LENGTH);

        // `pattern` is also the pattern control's key, which an authored
        // write records as JSON text; the array form is the one people write.
        let pattern = match config.get("pattern").and_then(|value| value.as_str()) {
            Some(json) => parse_pattern(Some(&serde_json::from_str(json)?), "pattern"),
            None => parse_pattern(config.get("pattern"), "pattern"),
        }
        .map_err(|error| error.refused_by(&reader))?;

        let cells = Arc::new(ControlCells::new(controls::defaults()));
        cells.publish(controls::ROOT_NOTE, RtValue::I32(i32::from(root_note)));
        let step_count = step_count.min(controls::MAX_STEPS as usize) as i32;
        cells.publish(controls::STEP_COUNT, RtValue::I32(step_count));
        cells.publish(controls::GATE_LENGTH, RtValue::F32(gate_length));
        if let Some(seconds) = reader.float(&GRACE_DURATION)? {
            cells.publish(controls::GRACE_DURATION, RtValue::F32(seconds));
        }
        let surface = DeclaredSurface::new(controls::TABLE.clone(), cells.clone());
        surface.show_payload("pattern", controls::pattern_json(&pattern).into());
        for key in ["mode", "grace_placement"] {
            if let Some(text) = config.get(key).and_then(|v| v.as_str()) {
                surface.set_control(key, text.into())?;
            }
        }
        let seq = StepSequencer::with_parts(sample_rate, cells, Shared::new(pattern));

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(seq)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}
