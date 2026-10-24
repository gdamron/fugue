//! Builds a cell sequencer from its config.

use std::sync::Arc;

use serde_json::Value;

use super::*;
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};

pub struct CellSequencerFactory;

const TYPE_ID: &str = "cell_sequencer";
const ROOT_NOTE: ConfigKey = ConfigKey::int::<u8>("root_note");
const STEP_COUNT: ConfigKey = ConfigKey::int::<usize>("step_count");
const GATE_LENGTH: ConfigKey = ConfigKey::float("gate_length");
// `start_cell` is the authored spelling of the initial cell. `select_cell` is
// the control's key, which an authored write records and which wins as the
// later write (until FUG-313 makes selecting a request that is never recorded).
const START_CELL: ConfigKey = ConfigKey::int::<usize>("start_cell");
const SELECT_CELL: ConfigKey = ConfigKey::int::<usize>("select_cell");
const GRACE_DURATION: ConfigKey = ConfigKey::float("grace_duration");
const GRACE_VELOCITY: ConfigKey = ConfigKey::float("grace_velocity");

impl ModuleFactory for CellSequencerFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const {
            &[
                ROOT_NOTE,
                STEP_COUNT,
                GATE_LENGTH,
                START_CELL,
                SELECT_CELL,
                GRACE_DURATION,
                GRACE_VELOCITY,
                ConfigKey::boolean("wait_for_cycle_end"),
                ConfigKey::json("cells"),
                ConfigKey::boolean("follow_cell_length"),
                ConfigKey::text("mode"),
                ConfigKey::text("grace_placement"),
            ]
        }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let root_note = reader.int::<u8>(&ROOT_NOTE)?.unwrap_or(DEFAULT_ROOT_NOTE);
        let step_count = reader.int::<usize>(&STEP_COUNT)?.unwrap_or(DEFAULT_STEPS);
        let gate_length = reader.float(&GATE_LENGTH)?.unwrap_or(DEFAULT_GATE_LENGTH);
        let selected_cell = match reader.int::<usize>(&SELECT_CELL)? {
            Some(cell) => cell,
            None => reader.int::<usize>(&START_CELL)?.unwrap_or(0),
        };
        let wait_for_cycle_end = config
            .get("wait_for_cycle_end")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        // `cells` is also the cells control's key, which an authored write
        // records as JSON text; the array form is the one people write.
        let sequences = match config.get("cells").and_then(|value| value.as_str()) {
            Some(json) => parse_sequence_bank(Some(&serde_json::from_str(json)?), "cells"),
            None => parse_sequence_bank(config.get("cells"), "cells"),
        }
        .map_err(|error| error.refused_by(&reader))?;

        let controls = CellSequencerControls::new_with_values(
            root_note,
            step_count,
            gate_length,
            selected_cell,
            wait_for_cycle_end,
            sequences.clone(),
        );
        if let Some(follow_cell_length) = config
            .get("follow_cell_length")
            .and_then(|value| value.as_bool())
        {
            controls.set_follow_cell_length(follow_cell_length);
        }
        if let Some(mode) = config.get("mode").and_then(|value| value.as_str()) {
            controls.set_mode(mode)?;
        }
        if let Some(seconds) = reader.float(&GRACE_DURATION)? {
            controls.set_grace_duration(seconds);
        }
        if let Some(scale) = reader.float(&GRACE_VELOCITY)? {
            controls.set_grace_velocity(scale);
        }
        if let Some(placement) = config
            .get("grace_placement")
            .and_then(|value| value.as_str())
        {
            controls.set_grace_placement(placement)?;
        }
        let module = CellSequencer::new_with_controls(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}
