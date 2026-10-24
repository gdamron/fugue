//! Builds a cell sequencer from its config.

use super::*;
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};
use crate::ControlSurface;

pub struct CellSequencerFactory;

const TYPE_ID: &str = "cell_sequencer";
const ROOT_NOTE_KEY: ConfigKey = ConfigKey::int::<u8>("root_note");
const STEP_COUNT_KEY: ConfigKey = ConfigKey::int::<usize>("step_count");
const GATE_LENGTH_KEY: ConfigKey = ConfigKey::float("gate_length");
// `start_cell` is the authored spelling of the initial cell. `select_cell` is
// the control's key, which an authored selection records and which wins as
// the later write: a saved document starts on the cell its author selected.
const START_CELL_KEY: ConfigKey = ConfigKey::int::<usize>("start_cell");
const SELECT_CELL_KEY: ConfigKey = ConfigKey::int::<usize>("select_cell");
const GRACE_DURATION_KEY: ConfigKey = ConfigKey::float("grace_duration");
const GRACE_VELOCITY_KEY: ConfigKey = ConfigKey::float("grace_velocity");

impl ModuleFactory for CellSequencerFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const {
            &[
                ROOT_NOTE_KEY,
                STEP_COUNT_KEY,
                GATE_LENGTH_KEY,
                START_CELL_KEY,
                SELECT_CELL_KEY,
                GRACE_DURATION_KEY,
                GRACE_VELOCITY_KEY,
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
        let root_note = reader
            .int::<u8>(&ROOT_NOTE_KEY)?
            .unwrap_or(DEFAULT_ROOT_NOTE);
        let step_count = reader
            .int::<usize>(&STEP_COUNT_KEY)?
            .unwrap_or(DEFAULT_STEPS);
        let gate_length = reader
            .float(&GATE_LENGTH_KEY)?
            .unwrap_or(DEFAULT_GATE_LENGTH);
        let start_cell = match reader.int::<usize>(&SELECT_CELL_KEY)? {
            Some(cell) => cell,
            None => reader.int::<usize>(&START_CELL_KEY)?.unwrap_or(0),
        };
        // `cells` is also the cells control's key, which an authored write
        // records as JSON text; the array form is the one people write.
        let bank = match config.get("cells").and_then(|value| value.as_str()) {
            Some(json) => parse_sequence_bank(Some(&serde_json::from_str(json)?), "cells"),
            None => parse_sequence_bank(config.get("cells"), "cells"),
        }
        .map_err(|error| error.refused_by(&reader))?;

        let cells = Arc::new(ControlCells::new(table::defaults()));
        cells.publish(ROOT_NOTE, RtValue::I32(i32::from(root_note)));
        let step_count = step_count.min(MAX_STEPS) as i32;
        cells.publish(STEP_COUNT, RtValue::I32(step_count));
        cells.publish(GATE_LENGTH, RtValue::F32(gate_length));
        for (key, control) in [
            ("wait_for_cycle_end", WAIT_FOR_CYCLE_END),
            ("follow_cell_length", FOLLOW_CELL_LENGTH),
        ] {
            if let Some(flag) = config.get(key).and_then(|value| value.as_bool()) {
                cells.publish(control, RtValue::Bool(flag));
            }
        }
        if let Some(seconds) = reader.float(&GRACE_DURATION_KEY)? {
            cells.publish(GRACE_DURATION, RtValue::F32(seconds));
        }
        if let Some(scale) = reader.float(&GRACE_VELOCITY_KEY)? {
            cells.publish(GRACE_VELOCITY, RtValue::F32(scale));
        }
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        surface.show_payload("cells", table::bank_json(&bank).into());
        for key in ["mode", "grace_placement"] {
            if let Some(text) = config.get(key).and_then(|value| value.as_str()) {
                surface.set_control(key, text.into())?;
            }
        }
        let seq = CellSequencer::with_parts(sample_rate, cells, Shared::new(bank), start_cell);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(seq)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}
