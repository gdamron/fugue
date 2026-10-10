use std::any::Any;
use std::sync::Arc;

use crate::control_request::{
    local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable, Refusal,
    RtValue,
};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};
use crate::{ControlMeta, Module};

use self::controls::{ENABLED, TABLE, TICK_RATE as TICK_RATE_CONTROL};

pub use self::controls::CodeControls;

mod controls;
mod inputs;
mod outputs;

/// Factory for the orchestration-only `code` module type.
///
/// The module itself does not generate audio. It anchors a script into the
/// graph and exposes a control surface used by the platform-specific script
/// host. Scripts may define plain top-level `init`, `tick`, and `reset`
/// functions, return a lifecycle object as their final expression, or use the
/// legacy `globalThis.*` hook style.
pub struct CodeFactory;

struct CodeConfig {
    script: String,
    entrypoint: Option<String>,
    enabled: bool,
    tick_rate: f32,
}

const TYPE_ID: &str = "code";
const TICK_RATE: ConfigKey = ConfigKey::float("tick_rate");

impl ModuleFactory for CodeFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const {
            &[
                TICK_RATE,
                ConfigKey::text("script"),
                ConfigKey::text("entrypoint"),
                ConfigKey::boolean("enabled"),
                // The script's own parameters (In C's `mixer_id`, say), an
                // object it reads from its module's config.
                ConfigKey::json("params"),
            ]
        }
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let config = parse_config(config)?;
        let controls = CodeControls::new(
            config.enabled,
            config.tick_rate,
            config.script,
            config.entrypoint,
        );

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(CodeModule {
                cells: controls.cells(),
            })),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

/// Parses the minimal v1 config accepted by the `code` module.
fn parse_config(config: &serde_json::Value) -> Result<CodeConfig, Box<dyn std::error::Error>> {
    if config
        .get("params")
        .is_some_and(|params| !params.is_object())
    {
        return Err("code config 'params' must be an object".into());
    }
    let script = config
        .get("script")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    let entrypoint = config
        .get("entrypoint")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let enabled = config
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let tick_rate = ConfigReader::new(TYPE_ID, config)
        .float(&TICK_RATE)?
        .unwrap_or(0.0);

    Ok(CodeConfig {
        script,
        entrypoint,
        enabled,
        tick_rate,
    })
}

/// Graph-resident shell for orchestration scripts.
///
/// This module intentionally performs no DSP work in `process()`. Script
/// execution happens on a host-managed thread or in the surrounding JS host for
/// wasm builds. It holds its declared controls' cells, which the host reads,
/// and nothing of the control-side strings or their lock.
pub struct CodeModule {
    cells: Arc<ControlCells>,
}

impl Module for CodeModule {
    fn name(&self) -> &str {
        "Code"
    }

    fn process(&mut self, _frames: usize) -> bool {
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, _index: usize) -> &mut [f32] {
        &mut []
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &[]
    }

    fn set_input(&mut self, port: &str, _value: f32) -> Result<(), String> {
        Err(format!("Unknown input port: {}", port))
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("Unknown output port: {}", port))
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    /// Both controls are held in their cells for the script host.
    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (control, value) {
            (ENABLED, RtValue::Bool(_)) => Ok(value),
            (TICK_RATE_CONTROL, RtValue::F32(rate)) => Ok(RtValue::F32(rate.max(0.0))),
            _ => Err(Refusal::Unsupported),
        }
    }

    fn controls(&self) -> Vec<ControlMeta> {
        local_controls(self)
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        local_get(self, key)
    }

    fn set_control(&mut self, key: &str, value: f32) -> Result<(), String> {
        local_set(self, key, value)
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_config, CodeFactory};
    use crate::ModuleFactory;

    #[test]
    fn code_factory_builds_module() {
        let config = serde_json::json!({
            "script": "graph.status()",
            "enabled": true,
            "tick_rate": 2.0
        });
        let built = CodeFactory.build(48_000, &config).unwrap();
        assert!(built.control_surface.is_some());
        assert_eq!(built.module.module().inputs().len(), 0);
    }

    #[test]
    fn code_params_are_an_object_and_top_level_keys_are_refused() {
        let registry = crate::ModuleRegistry::default();
        let params = serde_json::json!({ "params": { "mixer_id": "mixer", "sections": [1, 2] } });
        assert!(registry.build("code", 48_000, &params).is_ok());
        let error = registry
            .build("code", 48_000, &serde_json::json!({ "params": [1, 2] }))
            .err()
            .unwrap();
        assert!(
            error.to_string().contains("'params' must be an object"),
            "{error}"
        );
        let error = registry
            .build("code", 48_000, &serde_json::json!({ "mixer_id": "mixer" }))
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("code config has no key 'mixer_id'"),
            "{error}"
        );
    }

    #[test]
    fn code_config_defaults() {
        let config = parse_config(&serde_json::Value::Null).unwrap();
        assert!(config.enabled);
        assert_eq!(config.tick_rate, 0.0);
    }
}
