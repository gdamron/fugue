//! Thread-safe controls for the orchestration-only Agent module.
//!
//! These controls are the bridge between graph/UI APIs and the background agent
//! worker. User-facing controls configure prompts and expose results, while
//! `trigger_count` and `reset_count` are internal counters incremented by the
//! audio module and polled by the worker.

use std::sync::{Arc, Mutex};

use crate::{ControlMeta, ControlSurface, ControlValue};

/// Shared runtime state for an `agent` module.
///
/// The type is cloneable so the audio graph, runtime APIs, and background
/// worker can all hold handles to the same state. Access is mutex-protected
/// because it is never used for DSP math; the audio module only performs small
/// counter updates.
#[derive(Clone)]
pub struct AgentControls {
    shared: Arc<Mutex<AgentState>>,
}

#[derive(Clone, Debug)]
struct AgentState {
    enabled: bool,
    status: String,
    last_error: String,
    prompt: String,
    system: String,
    backend: String,
    last_response: String,
    last_response_json: String,
    history_json: String,
    last_apply_error: String,
    request_count: u64,
    trigger_count: u64,
    reset_count: u64,
    cooldown_ms: f32,
}

impl AgentControls {
    pub fn new(
        enabled: bool,
        prompt: String,
        system: String,
        backend: String,
        cooldown_ms: f32,
    ) -> Self {
        Self {
            shared: Arc::new(Mutex::new(AgentState {
                enabled,
                status: "idle".to_string(),
                last_error: String::new(),
                prompt,
                system,
                backend,
                last_response: String::new(),
                last_response_json: String::new(),
                history_json: "[]".to_string(),
                last_apply_error: String::new(),
                request_count: 0,
                trigger_count: 0,
                reset_count: 0,
                cooldown_ms: cooldown_ms.max(0.0),
            })),
        }
    }

    /// Records a rising edge on the `trigger` input.
    ///
    /// The background worker observes this monotonically increasing counter and
    /// services each new value outside the audio thread.
    pub fn increment_trigger(&self) {
        let mut state = self.shared.lock().unwrap();
        state.trigger_count = state.trigger_count.saturating_add(1);
    }

    /// Records a rising edge on the `reset` input.
    ///
    /// The worker uses this counter to clear history and errors without doing
    /// that allocation-heavy work in [`crate::Module::process`].
    pub fn increment_reset(&self) {
        let mut state = self.shared.lock().unwrap();
        state.reset_count = state.reset_count.saturating_add(1);
    }

    fn snapshot(&self) -> AgentState {
        self.shared.lock().unwrap().clone()
    }
}

impl ControlSurface for AgentControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let state = self.snapshot();
        vec![
            ControlMeta::boolean("enabled", "Enable or disable agent requests", state.enabled),
            ControlMeta::string("status", "Current agent runtime status")
                .with_default(state.status),
            ControlMeta::string("last_error", "Last agent runtime error")
                .with_default(state.last_error),
            ControlMeta::string("prompt", "User prompt template").with_default(state.prompt),
            ControlMeta::string("system", "System prompt").with_default(state.system),
            ControlMeta::string("backend", "Agent backend").with_default(state.backend),
            ControlMeta::string("last_response", "Last raw agent response")
                .with_default(state.last_response),
            ControlMeta::string("last_response_json", "Last parsed JSON response")
                .with_default(state.last_response_json),
            ControlMeta::string("history_json", "Bounded request/response history")
                .with_default(state.history_json),
            ControlMeta::string("last_apply_error", "Last graph apply error")
                .with_default(state.last_apply_error),
            ControlMeta::number("request_count", "Completed request count")
                .with_range(0.0, f32::MAX)
                .with_default(state.request_count as f32),
            ControlMeta::number(
                "cooldown_ms",
                "Minimum time between requests in milliseconds",
            )
            .with_range(0.0, f32::MAX)
            .with_default(state.cooldown_ms),
            ControlMeta::number("trigger_count", "Internal trigger edge counter")
                .with_range(0.0, f32::MAX)
                .with_default(state.trigger_count as f32),
            ControlMeta::number("reset_count", "Internal reset edge counter")
                .with_range(0.0, f32::MAX)
                .with_default(state.reset_count as f32),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        let state = self.shared.lock().unwrap();
        match key {
            "enabled" => Ok(state.enabled.into()),
            "status" => Ok(state.status.clone().into()),
            "last_error" => Ok(state.last_error.clone().into()),
            "prompt" => Ok(state.prompt.clone().into()),
            "system" => Ok(state.system.clone().into()),
            "backend" => Ok(state.backend.clone().into()),
            "last_response" => Ok(state.last_response.clone().into()),
            "last_response_json" => Ok(state.last_response_json.clone().into()),
            "history_json" => Ok(state.history_json.clone().into()),
            "last_apply_error" => Ok(state.last_apply_error.clone().into()),
            "request_count" => Ok((state.request_count as f32).into()),
            "trigger_count" => Ok((state.trigger_count as f32).into()),
            "reset_count" => Ok((state.reset_count as f32).into()),
            "cooldown_ms" => Ok(state.cooldown_ms.into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let mut state = self.shared.lock().unwrap();
        match key {
            "enabled" => state.enabled = value.as_bool()?,
            "status" => state.status = value.as_string()?.to_string(),
            "last_error" => state.last_error = value.as_string()?.to_string(),
            "prompt" => state.prompt = value.as_string()?.to_string(),
            "system" => state.system = value.as_string()?.to_string(),
            "backend" => state.backend = value.as_string()?.to_string(),
            "last_response" => state.last_response = value.as_string()?.to_string(),
            "last_response_json" => state.last_response_json = value.as_string()?.to_string(),
            "history_json" => state.history_json = value.as_string()?.to_string(),
            "last_apply_error" => state.last_apply_error = value.as_string()?.to_string(),
            "request_count" => state.request_count = value.as_number()?.max(0.0) as u64,
            "trigger_count" => state.trigger_count = value.as_number()?.max(0.0) as u64,
            "reset_count" => state.reset_count = value.as_number()?.max(0.0) as u64,
            "cooldown_ms" => state.cooldown_ms = value.as_number()?.max(0.0),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }
}
