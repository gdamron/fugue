//! Building a mixer from its config: its channel count, master, whole
//! `levels` and `pans`, and any channel's own level or pan a document
//! recorded, on its declared table.

use std::sync::Arc;

use crate::control_request::{apply_declared, RtValue};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::invention::declared::DeclaredSurface;
use crate::module_config::{finite_f32, ConfigKey, ConfigReader};

use super::controls::{DEFAULT_LEVEL, DEFAULT_PAN, TABLE};
use super::Mixer;

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
// Each array sets every channel's level or pan, as a cold build plays it:
// a channel's own `level.N` or `pan.N` beside it is applied after it.
const LEVELS: ConfigKey = ConfigKey::json("levels").replacing(&["level"]);
const PANS: ConfigKey = ConfigKey::json("pans").replacing(&["pan"]);

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

        let mut mixer = Mixer::new(channels).with_sample_rate(sample_rate);
        mixer.set_master(master);
        for ch in 0..mixer.channels {
            mixer.set_level(ch, levels.get(ch).copied().unwrap_or(DEFAULT_LEVEL));
            mixer.set_pan(ch, pans.get(ch).copied().unwrap_or(DEFAULT_PAN));
        }
        // A channel's own level or pan, as a document records a write.
        for (key, value) in config.as_object().into_iter().flatten() {
            let Some(channel) = ["level.", "pan."]
                .iter()
                .find_map(|prefix| Some((*prefix, key.strip_prefix(prefix)?)))
            else {
                continue;
            };
            let unknown = || format!("config '{key}': Unknown control: {key}");
            let index = channel.1.parse::<usize>().map_err(|_| unknown())?;
            let named = format!("{}{index}", channel.0);
            let control = TABLE
                .resolve_present(&named, |index| mixer.cells.load(index))
                .map_err(|refusal| format!("config '{key}': {refusal}"))?;
            let value = channel_number(key, value)?;
            let _ = apply_declared(&mut mixer, control, RtValue::F32(value));
        }
        let surface = DeclaredSurface::new(TABLE.clone(), mixer.cells.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(mixer)),
            handles: Vec::new(),
            control_surface: Some(Arc::new(surface)),
            sink: None,
        })
    }
}

/// A channel's level or pan as its config writes it: a finite number, or
/// one as text.
fn channel_number(key: &str, value: &serde_json::Value) -> Result<f32, String> {
    let not_numeric = || format!("config '{key}': Expected numeric control value");
    match value {
        serde_json::Value::Number(_) => {
            finite_f32(value).map_err(|refusal| format!("config '{key}' {refusal}"))
        }
        serde_json::Value::String(text) => match text.trim().parse::<f32>() {
            Ok(number) if number.is_finite() => Ok(number),
            Ok(_) => Err(format!(
                "config '{key}' expects a finite number, got {value}"
            )),
            Err(_) => Err(not_numeric()),
        },
        serde_json::Value::Bool(_) => Err(not_numeric()),
        _ => Err(format!("config '{key}' must be a number, boolean or text")),
    }
}
