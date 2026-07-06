use super::*;

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

impl ModuleFactory for StepSequencerFactory {
    fn type_id(&self) -> &'static str {
        "step_sequencer"
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let base_note = config
            .get("base_note")
            .and_then(|v| v.as_u64())
            .map(|v| v as u8)
            .unwrap_or(DEFAULT_BASE_NOTE);

        let steps = config
            .get("steps")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_STEPS);

        let gate_length = config
            .get("gate_length")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(DEFAULT_GATE_LENGTH);

        let pattern = parse_pattern(config.get("pattern"))?;

        let controls = StepSequencerControls::new_with_values(base_note, steps, gate_length);
        if let Some(mode) = config.get("mode").and_then(|v| v.as_str()) {
            controls.set_mode(mode)?;
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
