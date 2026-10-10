//! Orchestration-only agent module.
//!
//! An agent is a graph-resident trigger point for LLM-backed orchestration. It
//! has normal Fugue input ports, so clocks, sequencers, or scripts can trigger
//! it, but it performs no LLM work in [`Module::process`]. Instead, trigger and
//! reset edges, from its inputs or written as event controls, are counted on
//! shared atomic counters that the runtime [`crate::agents::AgentManager`]
//! drains on background threads.

use std::any::Any;
use std::sync::Arc;

use crate::control_request::{
    local_controls, local_get, local_set, ControlCells, ControlIndex, ControlTable, Refusal,
    RtValue,
};
use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::module_config::{ConfigKey, ConfigReader};
use crate::{ControlMeta, Module};

use self::controls::{Edges, COOLDOWN as COOLDOWN_CONTROL, ENABLED, RESET, TABLE, TRIGGER};

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
    system_prompt: String,
    backend: String,
    /// Seconds.
    cooldown: f32,
}

const TYPE_ID: &str = "agent";
const COOLDOWN: ConfigKey = ConfigKey::float("cooldown");

impl ModuleFactory for AgentFactory {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    fn config_keys(&self) -> &'static [ConfigKey] {
        const {
            &[
                COOLDOWN,
                ConfigKey::boolean("enabled"),
                ConfigKey::text("prompt"),
                ConfigKey::text("system_prompt"),
                ConfigKey::text("backend"),
                // Read by the agent host (src/agents) as it runs.
                ConfigKey::json("response"),
                // `max_turns` and `max_chars` of the history sent with each
                // request (the `history` telemetry is the record itself).
                ConfigKey::json("history_limits"),
                ConfigKey::json("context"),
                ConfigKey::boolean("include_graph_summary"),
                ConfigKey::json("apply"),
                ConfigKey::json("test_response"),
                ConfigKey::text("command"),
                ConfigKey::json("args"),
                ConfigKey::text("provider"),
                ConfigKey::text("model"),
                ConfigKey::json("max_tokens"),
            ]
        }
    }

    fn build(
        &self,
        _sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let config = parse_config(config)?;
        let controls = AgentControls::new(
            config.enabled,
            config.prompt,
            config.system_prompt,
            config.backend,
            config.cooldown,
        );

        let (cells, edges) = controls.audio_parts();
        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(AgentModule {
                cells,
                edges,
                inputs: inputs::AgentInputs::new(),
                last_trigger: 0.0,
                last_reset: 0.0,
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

fn parse_config(config: &serde_json::Value) -> Result<AgentConfig, Box<dyn std::error::Error>> {
    // Telemetry is the worker's to write, never authored. `history` was the
    // history settings' key before they became `history_limits`.
    if let Some(key) = controls::TELEMETRY
        .iter()
        .find(|key| config.get(**key).is_some())
    {
        let hint = if *key == "history" {
            "; the history settings are history_limits"
        } else {
            ""
        };
        return Err(format!("agent config can't set read-only telemetry '{key}'{hint}").into());
    }
    Ok(AgentConfig {
        enabled: config
            .get("enabled")
            .and_then(|value| value.as_bool())
            .unwrap_or(true),
        prompt: config
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        system_prompt: config
            .get("system_prompt")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        backend: config
            .get("backend")
            .and_then(|value| value.as_str())
            .unwrap_or("local:auto")
            .to_string(),
        cooldown: ConfigReader::new(TYPE_ID, config)
            .float(&COOLDOWN)?
            .unwrap_or(0.0),
    })
}

/// Audio-graph shell for an agent worker.
///
/// This module intentionally has no outputs and no heavy processing. Its only
/// audio-rate behavior is rising-edge detection for `trigger` and `reset`,
/// counted on lock-free atomics, and applying its declared controls. It
/// holds no handle on the control-side strings or their lock.
pub struct AgentModule {
    cells: Arc<ControlCells>,
    edges: Arc<Edges>,
    inputs: inputs::AgentInputs,
    last_trigger: f32,
    last_reset: f32,
}

impl Module for AgentModule {
    fn name(&self) -> &str {
        "Agent"
    }

    fn process(&mut self, frames: usize) -> bool {
        for i in 0..frames {
            let trigger = self.inputs.trigger(i);
            let reset = self.inputs.reset(i);
            if trigger > 0.5 && self.last_trigger <= 0.5 {
                self.edges.trigger.record();
            }
            if reset > 0.5 && self.last_reset <= 0.5 {
                self.edges.reset.record();
            }
            self.last_trigger = trigger;
            self.last_reset = reset;
        }
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, _index: usize) -> &[f32] {
        &[]
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        Err(format!("Unknown output port: {}", port))
    }

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    /// `enabled` and `cooldown` are held in their cells for the worker; an
    /// event fires an edge and holds nothing.
    #[allow(private_interfaces)]
    fn apply(&mut self, control: ControlIndex, value: RtValue) -> Result<RtValue, Refusal> {
        match (control, value) {
            (ENABLED, RtValue::Bool(_)) => Ok(value),
            (COOLDOWN_CONTROL, RtValue::F32(seconds)) => Ok(RtValue::F32(seconds.max(0.0))),
            (TRIGGER | RESET, RtValue::Bool(fire)) => {
                if fire {
                    match control {
                        TRIGGER => self.edges.trigger.record(),
                        _ => self.edges.reset.record(),
                    }
                }
                Ok(RtValue::Bool(false))
            }
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
    use super::{AgentControls, AgentFactory};
    use crate::ModuleFactory;

    #[test]
    fn edges_are_counted_without_allocating_or_locking() {
        let mut built = AgentFactory.build(48_000, &serde_json::json!({})).unwrap();
        let controls = built.handles[0]
            .1
            .downcast_ref::<AgentControls>()
            .unwrap()
            .clone();
        let module = built.module.module_mut();
        // Both inputs, `trigger` and `reset`: a rising edge every 8 frames.
        for index in 0..2 {
            for (i, sample) in module.input_block_mut(index).iter_mut().enumerate() {
                *sample = if i % 8 < 4 { 1.0 } else { 0.0 };
            }
        }
        let frames = crate::MAX_BLOCK;
        let edges = frames.div_ceil(8) as u32;

        let (_, allocs, frees) = crate::alloc_counter::allocator_events(|| module.process(frames));
        assert_eq!((allocs, frees), (0, 0));
        assert_eq!(
            (controls.trigger_count(), controls.reset_count()),
            (edges, edges)
        );

        // With the control thread holding the state lock, the audio path
        // still finishes: it never takes it.
        let held = controls.hold_state_lock();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let mut module = built.module;
        let audio = std::thread::spawn(move || {
            module.module_mut().process(frames);
            done_tx.send(()).unwrap();
        });
        let finished = done_rx.recv_timeout(std::time::Duration::from_secs(5));
        drop(held);
        audio.join().unwrap();
        assert!(finished.is_ok(), "process() waited on the state lock");
    }

    #[test]
    fn agent_factory_builds_module() {
        let built = AgentFactory
            .build(
                48_000,
                &serde_json::json!({
                    "prompt": "Generate a variation",
                    "backend": "test:echo",
                    "cooldown": 0.25
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
        assert_eq!(
            controls.get_control("cooldown").unwrap(),
            crate::ControlValue::Number(0.25)
        );
    }

    #[test]
    fn config_refuses_telemetry_and_names_history_limits() {
        let registry = crate::ModuleRegistry::default();
        let limits = serde_json::json!({ "history_limits": { "max_turns": 2, "max_chars": 500 } });
        assert!(registry.build("agent", 48_000, &limits).is_ok());
        let old = serde_json::json!({ "history": { "max_turns": 2 } });
        let error = registry
            .build("agent", 48_000, &old)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error
                .contains("read-only telemetry 'history'; the history settings are history_limits"),
            "{error}"
        );
        let error = registry
            .build("agent", 48_000, &serde_json::json!({ "status": "idle" }))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("read-only telemetry 'status'"), "{error}");
    }
}
