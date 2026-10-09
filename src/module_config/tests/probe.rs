//! Test-only factories with a declared integer key, for checking the
//! registry harness and the entry points on an integer key before any
//! built-in module declares one.

use crate::factory::{ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};

/// An oscillator whose frequency is the whole number `hz`, read by the
/// reader. Its other number controls are its `frequency_mod_depth` key and the
/// control keys it applies.
pub(crate) struct ProbeFactory;

/// Like [`ProbeFactory`], but reading `hz` as factories did before the
/// reader: a float spelling falls back to the default.
pub(crate) struct LegacyProbeFactory;

const HZ: ConfigKey = ConfigKey::int::<u16>("hz");
const FREQUENCY_MOD_DEPTH: ConfigKey = ConfigKey::float("frequency_mod_depth");

impl ModuleFactory for ProbeFactory {
    fn type_id(&self) -> &'static str {
        "probe"
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        &[HZ, FREQUENCY_MOD_DEPTH]
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
        &[HZ, FREQUENCY_MOD_DEPTH]
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
    let fm_amount = reader.float(&FREQUENCY_MOD_DEPTH)?.unwrap_or(0.0);
    crate::modules::oscillator::built(
        sample_rate,
        [f32::from(hz), fm_amount, 0.0],
        config,
        |key| matches!(key, "frequency" | "amplitude_mod_depth" | "waveform"),
    )
}
