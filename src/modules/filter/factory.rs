//! Building filters from an invention's config.

use std::sync::Arc;

use super::controls::{self, CUTOFF, CUTOFF_MOD_DEPTH, RESONANCE, TABLE};
use super::Filter;
use crate::control_request::{ControlCells, RtValue};
use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{ConfigKey, ConfigReader};

/// Factory for constructing Filter modules from configuration.
pub struct FilterFactory;

const TYPE_ID: &str = "filter";
const CUTOFF_KEY: ConfigKey = ConfigKey::float("cutoff");
const RESONANCE_KEY: ConfigKey = ConfigKey::float("resonance");
const CUTOFF_MOD_DEPTH_KEY: ConfigKey = ConfigKey::float("cutoff_mod_depth");

impl ModuleFactory for FilterFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[CUTOFF_KEY, RESONANCE_KEY, CUTOFF_MOD_DEPTH_KEY]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let cutoff = reader.float(&CUTOFF_KEY)?.unwrap_or(1000.0);
        let resonance = reader.float(&RESONANCE_KEY)?.unwrap_or(0.0);
        let cutoff_mod_depth = reader.float(&CUTOFF_MOD_DEPTH_KEY)?.unwrap_or(5000.0);

        let cells = Arc::new(ControlCells::new(controls::defaults()));
        cells.publish(CUTOFF, RtValue::F32(cutoff));
        cells.publish(RESONANCE, RtValue::F32(resonance));
        cells.publish(CUTOFF_MOD_DEPTH, RtValue::F32(cutoff_mod_depth));
        let surface = DeclaredSurface::new(TABLE.clone(), cells.clone());
        // Only text names a filter type; anything else leaves the default,
        // as it always has.
        let named = config
            .get("filter_type")
            .is_some_and(|value| value.is_string());
        apply_control_keys(&surface, config, |key| named && key == "filter_type")?;
        let filter = Filter::with_cells(sample_rate, cells);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(filter)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}
