//! Thread-safe controls for the orchestration-only Agent module.
//!
//! These controls are the bridge between graph/UI APIs and the background agent
//! worker. User-facing controls configure prompts; telemetry (`status`,
//! `last_*`, `history`, `request_count`) is read-only through `set_control`
//! and written only by the worker. The trigger and reset edge counters are
//! internal: the audio module bumps them without locking, and the worker
//! polls them.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::traits::{check_listed_control, read_only, ControlSurfaceMap};
use crate::{ControlMeta, ControlSurface, ControlValue};

/// The controls only the agent's worker writes.
pub(super) const TELEMETRY: &[&str] = &[
    "status",
    "last_error",
    "last_response",
    "last_parsed_response",
    "history",
    "last_apply_error",
    "request_count",
];

/// Shared runtime state for an `agent` module.
///
/// The type is cloneable so the audio graph, runtime APIs, and background
/// worker can all hold handles to the same state. The string state is
/// mutex-protected because only control-thread code touches it; the audio
/// module only bumps the atomic edge counters.
#[derive(Clone)]
pub struct AgentControls {
    shared: Arc<Mutex<AgentState>>,
    edges: Arc<EdgeCounters>,
}

#[derive(Default)]
struct EdgeCounters {
    trigger: AtomicU64,
    reset: AtomicU64,
}

#[derive(Clone, Debug)]
struct AgentState {
    enabled: bool,
    status: String,
    last_error: String,
    prompt: String,
    system_prompt: String,
    backend: String,
    last_response: String,
    last_parsed_response: String,
    history: String,
    last_apply_error: String,
    request_count: u64,
    /// Seconds.
    cooldown: f32,
}

impl AgentControls {
    pub fn new(
        enabled: bool,
        prompt: String,
        system_prompt: String,
        backend: String,
        cooldown: f32,
    ) -> Self {
        Self {
            shared: Arc::new(Mutex::new(AgentState {
                enabled,
                status: "idle".to_string(),
                last_error: String::new(),
                prompt,
                system_prompt,
                backend,
                last_response: String::new(),
                last_parsed_response: String::new(),
                history: "[]".to_string(),
                last_apply_error: String::new(),
                request_count: 0,
                cooldown: cooldown.max(0.0),
            })),
            edges: Arc::default(),
        }
    }

    /// Records a rising edge on the `trigger` input. Audio thread: lock-free.
    ///
    /// The background worker observes this monotonically increasing counter and
    /// services each new value outside the audio thread.
    pub fn increment_trigger(&self) {
        self.edges.trigger.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a rising edge on the `reset` input. Audio thread: lock-free.
    ///
    /// The worker uses this counter to clear history and errors without doing
    /// that allocation-heavy work in [`crate::Module::process`].
    pub fn increment_reset(&self) {
        self.edges.reset.fetch_add(1, Ordering::Relaxed);
    }

    /// Rising edges seen on the `trigger` input so far.
    // The native agent worker reads it; wasm hosts have none yet.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn trigger_count(&self) -> u64 {
        self.edges.trigger.load(Ordering::Relaxed)
    }

    /// Rising edges seen on the `reset` input so far.
    // The native agent worker reads it; wasm hosts have none yet.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn reset_count(&self) -> u64 {
        self.edges.reset.load(Ordering::Relaxed)
    }

    /// Whether `other` is a handle on these same controls rather than on a
    /// replacement module's.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn same_instance(&self, other: &AgentControls) -> bool {
        Arc::ptr_eq(&self.edges, &other.edges)
    }

    /// Writes a telemetry control: the agent worker's writer, since
    /// [`ControlSurface::set_control`] refuses them. Refuses any other key.
    // The native agent worker reads it; wasm hosts have none yet.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn set_telemetry(&self, key: &str, value: ControlValue) -> Result<(), String> {
        let mut state = self.shared.lock().unwrap();
        match key {
            "status" => state.status = value.as_string()?.to_string(),
            "last_error" => state.last_error = value.as_string()?.to_string(),
            "last_response" => state.last_response = value.as_string()?.to_string(),
            "last_parsed_response" => state.last_parsed_response = value.as_string()?.to_string(),
            "history" => state.history = value.as_string()?.to_string(),
            "last_apply_error" => state.last_apply_error = value.as_string()?.to_string(),
            "request_count" => state.request_count = value.as_number()?.max(0.0) as u64,
            _ => return Err(format!("Not agent telemetry: {}", key)),
        }
        Ok(())
    }

    /// Holds the state lock, for a test proving the audio path never takes it.
    #[cfg(test)]
    pub(super) fn hold_state_lock(&self) -> std::sync::MutexGuard<'_, impl Sized> {
        self.shared.lock().unwrap()
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
            ControlMeta::string("status", "Current agent runtime status (read-only)")
                .with_default(state.status),
            ControlMeta::string("last_error", "Last agent runtime error (read-only)")
                .with_default(state.last_error),
            ControlMeta::string("prompt", "User prompt template").with_default(state.prompt),
            ControlMeta::string("system_prompt", "System prompt").with_default(state.system_prompt),
            ControlMeta::string("backend", "Agent backend").with_default(state.backend),
            ControlMeta::string("last_response", "Last raw agent response (read-only)")
                .with_default(state.last_response),
            ControlMeta::string(
                "last_parsed_response",
                "Last parsed JSON response (read-only)",
            )
            .with_default(state.last_parsed_response),
            ControlMeta::string("history", "Bounded request/response history (read-only)")
                .with_default(state.history),
            ControlMeta::string("last_apply_error", "Last graph apply error (read-only)")
                .with_default(state.last_apply_error),
            ControlMeta::number("request_count", "Completed request count (read-only)")
                .with_range(0.0, f32::MAX)
                .with_default(state.request_count as f32),
            ControlMeta::number("cooldown", "Minimum time between requests in seconds")
                .with_range(0.0, f32::MAX)
                .with_default(state.cooldown),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        let state = self.shared.lock().unwrap();
        match key {
            "enabled" => Ok(state.enabled.into()),
            "status" => Ok(state.status.clone().into()),
            "last_error" => Ok(state.last_error.clone().into()),
            "prompt" => Ok(state.prompt.clone().into()),
            "system_prompt" => Ok(state.system_prompt.clone().into()),
            "backend" => Ok(state.backend.clone().into()),
            "last_response" => Ok(state.last_response.clone().into()),
            "last_parsed_response" => Ok(state.last_parsed_response.clone().into()),
            "history" => Ok(state.history.clone().into()),
            "last_apply_error" => Ok(state.last_apply_error.clone().into()),
            "request_count" => Ok((state.request_count as f32).into()),
            "cooldown" => Ok(state.cooldown.into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        if TELEMETRY.contains(&key) {
            return read_only(key);
        }
        let mut state = self.shared.lock().unwrap();
        match key {
            "enabled" => state.enabled = value.as_bool()?,
            "prompt" => state.prompt = value.as_string()?.to_string(),
            "system_prompt" => state.system_prompt = value.as_string()?.to_string(),
            "backend" => state.backend = value.as_string()?.to_string(),
            "cooldown" => state.cooldown = value.as_number()?.max(0.0),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        _surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        if TELEMETRY.contains(&key) {
            return read_only(key);
        }
        check_listed_control(&self.controls(), key, value)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controls() -> AgentControls {
        AgentControls::new(true, String::new(), String::new(), "test:echo".into(), 0.0)
    }

    #[test]
    fn telemetry_is_read_only_through_set_control() {
        let controls = controls();
        let surfaces = ControlSurfaceMap::default();
        for key in TELEMETRY {
            let value = match controls.get_control(key).unwrap() {
                ControlValue::Number(_) => ControlValue::Number(7.0),
                _ => ControlValue::String("written".into()),
            };
            let before = controls.get_control(key).unwrap();
            let refusal = format!("Control '{key}' is read-only");
            assert_eq!(
                controls.set_control(key, value.clone()),
                Err(refusal.clone())
            );
            assert_eq!(
                controls.validate_control(key, &value, &surfaces),
                Err(refusal)
            );
            assert_eq!(controls.get_control(key).unwrap(), before, "{key}");
            // The worker's writer still lands it.
            controls.set_telemetry(key, value.clone()).unwrap();
            assert_eq!(controls.get_control(key).unwrap(), value, "{key}");
        }
    }

    #[test]
    fn telemetry_writer_refuses_parameters() {
        let controls = controls();
        let refused = controls.set_telemetry("prompt", ControlValue::String("x".into()));
        assert!(refused.is_err());
        assert_eq!(
            controls.get_control("prompt").unwrap(),
            String::new().into()
        );
    }

    #[test]
    fn edge_counters_are_internal() {
        let controls = controls();
        controls.increment_trigger();
        controls.increment_trigger();
        controls.increment_reset();
        assert_eq!((controls.trigger_count(), controls.reset_count()), (2, 1));
        assert!(controls.same_instance(&controls.clone()));
        assert!(!controls.same_instance(&self::controls()));
        for key in ["trigger_count", "reset_count"] {
            assert!(controls.get_control(key).is_err(), "{key}");
            assert!(controls.controls().iter().all(|meta| meta.key != key));
        }
    }
}
