//! Orchestration-only agent module.
//!
//! An agent is a graph-resident trigger point for LLM-backed orchestration. It
//! has normal Fugue input ports, so clocks, sequencers, or scripts can trigger
//! it, but it performs no LLM work in [`Module::process`]. Instead, trigger and
//! reset edges increment shared counters that are drained by the runtime
//! [`crate::agents::AgentManager`] on background threads.

use std::any::Any;
use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::{ControlMeta, ControlSurface, Module};

pub use self::controls::AgentControls;

mod controls;
mod inputs;
mod outputs;

/// Factory for constructing `agent` modules from invention config.
///
/// The factory stores orchestration config in the runtime snapshot and exposes a
/// shared [`AgentControls`] surface. The worker reads both immutable config and
/// mutable controls when servicing each trigger.
pub struct AgentFactory;

struct AgentConfig {
    enabled: bool,
    prompt: String,
    system: String,
    backend: String,
    cooldown_ms: f32,
}

impl ModuleFactory for AgentFactory {
    fn type_id(&self) -> &'static str {
        "agent"
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let config = parse_config(config);
        let controls = AgentControls::new(
            config.enabled,
            config.prompt,
            config.system,
            config.backend,
            config.cooldown_ms,
        );

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(AgentModule {
                controls: controls.clone(),
                inputs: inputs::AgentInputs::new(),
                last_trigger: 0.0,
                last_reset: 0.0,
                last_processed_sample: 0,
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

fn parse_config(config: &serde_json::Value) -> AgentConfig {
    AgentConfig {
        enabled: config
            .get("enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(true),
        prompt: config
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        system: config
            .get("system")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        backend: config
            .get("backend")
            .and_then(|value| value.as_str())
            .unwrap_or("local:auto")
            .to_string(),
        cooldown_ms: config
            .get("cooldown_ms")
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0) as f32,
    }
}

/// Audio-graph shell for an agent worker.
///
/// This module intentionally has no outputs and no heavy processing. Its only
/// audio-rate behavior is rising-edge detection for `trigger` and `reset`.
pub struct AgentModule {
    controls: AgentControls,
    inputs: inputs::AgentInputs,
    last_trigger: f32,
    last_reset: f32,
    last_processed_sample: u64,
}

impl Module for AgentModule {
    fn name(&self) -> &str {
        "Agent"
    }

    fn process(&mut self) -> bool {
        if self.inputs.trigger() > 0.5 && self.last_trigger <= 0.5 {
            self.controls.increment_trigger();
        }
        if self.inputs.reset() > 0.5 && self.last_reset <= 0.5 {
            self.controls.increment_reset();
        }
        self.last_trigger = self.inputs.trigger();
        self.last_reset = self.inputs.reset();
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("Unknown output port: {}", port))
    }

    fn last_processed_sample(&self) -> u64 {
        self.last_processed_sample
    }

    fn mark_processed(&mut self, sample: u64) {
        self.last_processed_sample = sample;
    }

    fn controls(&self) -> Vec<ControlMeta> {
        self.controls.controls()
    }
}

#[cfg(test)]
mod tests {
    use super::AgentFactory;
    use crate::ModuleFactory;

    #[test]
    fn agent_factory_builds_module() {
        let built = AgentFactory
            .build(
                48_000,
                &serde_json::json!({
                    "prompt": "Generate a variation",
                    "backend": "test:echo",
                    "cooldown_ms": 250
                }),
            )
            .unwrap();
        assert!(built.control_surface.is_some());
        assert_eq!(built.module.module().inputs(), &["trigger", "reset"]);
        let controls = built.control_surface.unwrap();
        assert_eq!(
            controls.get_control("backend").unwrap(),
            "test:echo".to_string().into()
        );
    }
}
