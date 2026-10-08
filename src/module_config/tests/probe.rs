//! Test-only factories with a declared integer key, for checking the
//! registry harness and the entry points on an integer key before any
//! built-in module declares one.

use crate::factory::{apply_control_keys, GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};
use crate::modules::{Oscillator, OscillatorControls, OscillatorType};
use std::sync::Arc;

/// An oscillator whose frequency is the whole number `hz`, read by the
/// reader. Its other number controls are its `fm_amount` key and the
/// control keys it applies.
pub(crate) struct ProbeFactory;

/// Like [`ProbeFactory`], but reading `hz` as factories did before the
/// reader: a float spelling falls back to the default.
pub(crate) struct LegacyProbeFactory;

const HZ: ConfigKey = ConfigKey::int::<u16>("hz");
const FM_AMOUNT: ConfigKey = ConfigKey::float("fm_amount");

impl ModuleFactory for ProbeFactory {
    fn type_id(&self) -> &'static str {
        "probe"
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[HZ, FM_AMOUNT]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let reader = ConfigReader::new("probe", config);
        let hz = reader.int::<u16>(&HZ)?.unwrap_or(440);
        build(sample_rate, config, hz)
    }
}

impl ModuleFactory for LegacyProbeFactory {
    fn type_id(&self) -> &'static str {
        "legacy_probe"
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[HZ, FM_AMOUNT]
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let hz = config.get("hz").and_then(|v| v.as_u64()).unwrap_or(440) as u16;
        build(sample_rate, config, hz)
    }
}

fn build(
    sample_rate: u32,
    config: &serde_json::Value,
    hz: u16,
) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
    let reader = ConfigReader::new("probe", config);
    let fm_amount = reader.float(&FM_AMOUNT)?.unwrap_or(0.0);
    let controls = OscillatorControls::new(f32::from(hz), OscillatorType::Sine, fm_amount, 0.0);
    apply_control_keys(&controls, config, |key| {
        matches!(key, "frequency" | "am_amount" | "type")
    })?;
    let module = Oscillator::new_with_controls(sample_rate, controls.clone());
    Ok(ModuleBuildResult {
        module: GraphModule::Module(Box::new(module)),
        handles: Vec::new(),
        control_surface: Some(Arc::new(controls)),
        sink: None,
    })
}
