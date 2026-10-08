use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};
use crate::{ControlMeta, ControlSurface, Module};

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
    tick_hz: f32,
}

const TYPE_ID: &str = "code";
const TICK_HZ: ConfigKey = ConfigKey::float("tick_hz");

impl ModuleFactory for CodeFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const {
            &[
                TICK_HZ,
                ConfigKey::text("script"),
                ConfigKey::text("entrypoint"),
                ConfigKey::boolean("enabled"),
            ]
        }
    }

    /// A script reads its own parameters from the top level of its config
    /// (In C's `mixer_id`, say) until they move under `params`.
    fn open_config(&self) -> bool {
        true
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let config = parse_config(config)?;
        let controls = CodeControls::new(
            config.enabled,
            config.tick_hz,
            config.script,
            config.entrypoint,
        );

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(CodeModule {
                controls: controls.clone(),
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
    let tick_hz = ConfigReader::new(TYPE_ID, config)
        .float(&TICK_HZ)?
        .unwrap_or(0.0);

    Ok(CodeConfig {
        script,
        entrypoint,
        enabled,
        tick_hz,
    })
}

/// Graph-resident shell for orchestration scripts.
///
/// This module intentionally performs no DSP work in `process()`. Script
/// execution happens on a host-managed thread or in the surrounding JS host for
/// wasm builds.
pub struct CodeModule {
    controls: CodeControls,
}

impl Module for CodeModule {
    fn name(&self) -> &str {
        "Code"
    }

    fn process(&mut self, _frames: usize) -> bool {
        let _ = &self.controls;
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

    fn controls(&self) -> Vec<ControlMeta> {
        self.controls.controls()
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
            "tick_hz": 2.0
        });
        let built = CodeFactory.build(48_000, &config).unwrap();
        assert!(built.control_surface.is_some());
        assert_eq!(built.module.module().inputs().len(), 0);
    }

    #[test]
    fn code_config_defaults() {
        let config = parse_config(&serde_json::Value::Null).unwrap();
        assert!(config.enabled);
        assert_eq!(config.tick_hz, 0.0);
    }
}
