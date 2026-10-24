//! Building a mixer from its config: its channel count, master, whole
//! `levels` and `pans`, and any channel's own level or pan a document
//! recorded.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};

use super::{Mixer, MixerControls};

/// Factory for constructing Mixer modules from configuration.
///
/// # Configuration Options
///
/// - `channel_count` (usize): Number of input channels, 1-64 (default: 4)
/// - `levels` (array of f32): Initial level for each channel (default: all 1.0)
/// - `pans` (array of f32): Initial pan position for each channel (-1.0 to 1.0, default: all 0.0)
/// - `master` (f32): Master output level (default: 1.0)
///
/// # Example
///
/// ```json
/// {
///   "id": "main_mixer",
///   "type": "mixer",
///   "config": {
///     "channel_count": 4,
///     "levels": [0.8, 0.6, 0.4, 0.3],
///     "pans": [-0.5, -0.15, 0.15, 0.5],
///     "master": 0.8
///   }
/// }
/// ```
pub struct MixerFactory;

const TYPE_ID: &str = "mixer";
const CHANNEL_COUNT: ConfigKey = ConfigKey::int::<usize>("channel_count");
const MASTER: ConfigKey = ConfigKey::float("master");
const LEVELS: ConfigKey = ConfigKey::json("levels");
const PANS: ConfigKey = ConfigKey::json("pans");

impl ModuleFactory for MixerFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const { &[CHANNEL_COUNT, MASTER, LEVELS, PANS] }
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new(TYPE_ID, config);
        let channels = reader.int::<usize>(&CHANNEL_COUNT)?.unwrap_or(4);
        let master = reader.float(&MASTER)?.unwrap_or(1.0);
        let levels = reader.floats(&LEVELS)?.unwrap_or_default();
        let pans = reader.floats(&PANS)?.unwrap_or_default();

        let controls = MixerControls::new_with_config(channels, &levels, &pans, master);
        crate::factory::apply_control_keys(&controls, config, |key| {
            key.starts_with("level.") || key.starts_with("pan.")
        })?;
        let mixer =
            Mixer::new_with_controls(channels, controls.clone()).with_sample_rate(sample_rate);

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(mixer)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}
