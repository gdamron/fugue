//! The agent's controls: its scalars are declared, and its prompts and
//! telemetry strings stay control-side.
//!
//! `enabled` and `cooldown` are declared parameters, applied by the audio
//! thread like any declared control and read back from their cells by the
//! background worker. Rising edges on the `trigger` and `reset` inputs are
//! counted on [`EventCounter`]s the audio module records and the worker
//! reads. `request_count` is declared telemetry, written only by the worker.
//!
//! The strings (`prompt`, `system_prompt`, `backend`, and the telemetry
//! `status`, `last_*` and `history`) sit behind a lock only control threads
//! take: the audio module holds the cells and the edge counters, never the
//! lock, so no audio-thread path reaches it.

use std::sync::{Arc, Mutex};

use crate::control_request::{
    Automation, ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind, EventCounter,
    RtValue,
};
use crate::invention::declared::{Declaration, DeclaredSurface, Route};
use crate::traits::{check_listed_control, read_only, ControlSurfaceMap};
use crate::{ControlMeta, ControlSurface, ControlValue, Module};

pub(super) const ENABLED: ControlIndex = ControlIndex(0);
pub(super) const COOLDOWN: ControlIndex = ControlIndex(1);
pub(super) const REQUEST_COUNT: ControlIndex = ControlIndex(2);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "enabled",
        DeclKind::Bool,
        RtValue::Bool(true),
        "Enable or disable agent requests",
    ),
    ControlDecl::new(
        "cooldown",
        DeclKind::Number {
            min: 0.0,
            max: f32::MAX,
        },
        RtValue::F32(0.0),
        "Minimum time between requests in seconds",
    )
    .unit("s")
    .clamped(0.0, f32::MAX),
    // Written by the agent's worker alone (see `ControlCells`).
    ControlDecl::new(
        "request_count",
        DeclKind::Number {
            min: 0.0,
            max: f32::MAX,
        },
        RtValue::F32(0.0),
        "Completed request count (read-only)",
    )
    .telemetry(),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

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

/// Rising edges on the `trigger` and `reset` inputs. The audio thread
/// records them; the worker reads them.
#[derive(Default)]
pub(super) struct Edges {
    pub(super) trigger: EventCounter,
    pub(super) reset: EventCounter,
}

/// Shared runtime state for an `agent` module.
///
/// The type is cloneable so the audio graph, runtime APIs, and background
/// worker can all hold handles to the same state. It is the agent's control
/// surface: declared controls go through [`DeclaredSurface`], the strings
/// through a lock only control threads take.
#[derive(Clone)]
pub struct AgentControls {
    declared: Arc<DeclaredSurface>,
    cells: Arc<ControlCells>,
    shared: Arc<Mutex<AgentState>>,
    edges: Arc<Edges>,
}

#[derive(Clone, Debug)]
struct AgentState {
    status: String,
    last_error: String,
    prompt: String,
    system_prompt: String,
    backend: String,
    last_response: String,
    last_parsed_response: String,
    history: String,
    last_apply_error: String,
}

impl AgentControls {
    pub fn new(
        enabled: bool,
        prompt: String,
        system_prompt: String,
        backend: String,
        cooldown: f32,
    ) -> Self {
        let cells = Arc::new(ControlCells::new(DECLS.iter().map(|decl| decl.default)));
        cells.publish(ENABLED, RtValue::Bool(enabled));
        cells.publish(COOLDOWN, RtValue::F32(cooldown.max(0.0)));
        Self {
            declared: Arc::new(DeclaredSurface::new(TABLE.clone(), cells.clone())),
            cells,
            shared: Arc::new(Mutex::new(AgentState {
                status: "idle".to_string(),
                last_error: String::new(),
                prompt,
                system_prompt,
                backend,
                last_response: String::new(),
                last_parsed_response: String::new(),
                history: "[]".to_string(),
                last_apply_error: String::new(),
            })),
            edges: Arc::default(),
        }
    }

    /// What the audio module holds: the cells and the edge counters, never
    /// the strings' lock.
    pub(super) fn audio_parts(&self) -> (Arc<ControlCells>, Arc<Edges>) {
        (self.cells.clone(), self.edges.clone())
    }

    /// Records a rising edge on the `trigger` input, as the audio module
    /// does. Lock-free.
    ///
    /// The background worker observes this monotonically increasing counter and
    /// services each new value outside the audio thread.
    pub fn increment_trigger(&self) {
        self.edges.trigger.record();
    }

    /// Records a rising edge on the `reset` input, as the audio module does.
    /// Lock-free.
    ///
    /// The worker uses this counter to clear history and errors without doing
    /// that allocation-heavy work in [`crate::Module::process`].
    pub fn increment_reset(&self) {
        self.edges.reset.record();
    }

    /// Rising edges seen on `trigger` so far, modulo 2^32. Seeing one also
    /// shows every control the audio thread applied before it.
    // The native agent worker reads it; wasm hosts have none yet.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn trigger_count(&self) -> u32 {
        self.edges.trigger.count()
    }

    /// Rising edges seen on `reset` so far, modulo 2^32.
    // The native agent worker reads it; wasm hosts have none yet.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn reset_count(&self) -> u32 {
        self.edges.reset.count()
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
        if key == "request_count" {
            // The worker is this cell's one writer; the module never writes it.
            let count = value.as_number()?.max(0.0);
            self.cells.publish(REQUEST_COUNT, RtValue::F32(count));
            return Ok(());
        }
        let mut state = self.shared.lock().unwrap();
        match key {
            "status" => state.status = value.as_string()?.to_string(),
            "last_error" => state.last_error = value.as_string()?.to_string(),
            "last_response" => state.last_response = value.as_string()?.to_string(),
            "last_parsed_response" => state.last_parsed_response = value.as_string()?.to_string(),
            "history" => state.history = value.as_string()?.to_string(),
            "last_apply_error" => state.last_apply_error = value.as_string()?.to_string(),
            _ => return Err(format!("Not agent telemetry: {}", key)),
        }
        Ok(())
    }

    /// Holds the strings' lock, for a test proving the audio path never
    /// takes it.
    #[cfg(test)]
    pub(crate) fn hold_state_lock(&self) -> std::sync::MutexGuard<'_, impl Sized> {
        self.shared.lock().unwrap()
    }

    fn snapshot(&self) -> AgentState {
        self.shared.lock().unwrap().clone()
    }
}

impl ControlSurface for AgentControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let [enabled, cooldown, request_count]: [ControlMeta; 3] = self
            .declared
            .controls()
            .try_into()
            .expect("the agent declares three controls");
        let state = self.snapshot();
        vec![
            enabled,
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
            request_count,
            cooldown,
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        if self.declared.declares(key) {
            return self.declared.get_control(key);
        }
        let state = self.shared.lock().unwrap();
        match key {
            "status" => Ok(state.status.clone().into()),
            "last_error" => Ok(state.last_error.clone().into()),
            "prompt" => Ok(state.prompt.clone().into()),
            "system_prompt" => Ok(state.system_prompt.clone().into()),
            "backend" => Ok(state.backend.clone().into()),
            "last_response" => Ok(state.last_response.clone().into()),
            "last_parsed_response" => Ok(state.last_parsed_response.clone().into()),
            "history" => Ok(state.history.clone().into()),
            "last_apply_error" => Ok(state.last_apply_error.clone().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        if self.declared.declares(key) {
            return self.declared.set_control(key, value);
        }
        if TELEMETRY.contains(&key) {
            return read_only(key);
        }
        let mut state = self.shared.lock().unwrap();
        match key {
            "prompt" => state.prompt = value.as_string()?.to_string(),
            "system_prompt" => state.system_prompt = value.as_string()?.to_string(),
            "backend" => state.backend = value.as_string()?.to_string(),
            _ => return Err(format!("Unknown control: {}", key)),
        }
        Ok(())
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        if self.declared.declares(key) {
            return self.declared.validate_control(key, value, surfaces);
        }
        if TELEMETRY.contains(&key) {
            return read_only(key);
        }
        check_listed_control(&self.controls(), key, value)
    }

    #[allow(private_interfaces)]
    fn bind(&self, route: Route, module: &mut dyn Module) {
        self.declared.bind(route, module);
    }

    fn set_legacy(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match self.declared.declares(key) {
            true => self.declared.set_legacy(key, value),
            false => self.set_control(key, value),
        }
    }

    #[allow(private_interfaces)]
    fn activate(&self, route: Route) {
        self.declared.activate(route);
    }

    fn retire(&self) {
        self.declared.retire();
    }

    fn declares(&self, key: &str) -> bool {
        self.declared.declares(key)
    }

    #[allow(private_interfaces)]
    fn declaration(&self, key: &str) -> Option<Declaration> {
        self.declared.declaration(key)
    }

    #[allow(private_interfaces)]
    fn automation(&self, key: &str) -> Option<Automation> {
        self.declared.automation(key)
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
        for (key, value) in [
            ("prompt", ControlValue::String("x".into())),
            ("enabled", ControlValue::Bool(false)),
            ("cooldown", ControlValue::Number(3.0)),
        ] {
            let before = controls.get_control(key).unwrap();
            assert!(controls.set_telemetry(key, value).is_err(), "{key}");
            assert_eq!(controls.get_control(key).unwrap(), before, "{key}");
        }
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

    #[test]
    fn controls_list_in_their_old_order() {
        let keys: Vec<String> = controls().controls().into_iter().map(|m| m.key).collect();
        assert_eq!(
            keys,
            [
                "enabled",
                "status",
                "last_error",
                "prompt",
                "system_prompt",
                "backend",
                "last_response",
                "last_parsed_response",
                "history",
                "last_apply_error",
                "request_count",
                "cooldown",
            ]
        );
    }

    #[test]
    fn only_enabled_and_cooldown_can_be_scheduled() {
        let controls = controls();
        for key in ["enabled", "cooldown"] {
            assert!(controls.automation(key).is_some(), "{key}");
        }
        // Declared but not automatable: a scheduler refuses it as it loads.
        assert!(controls.declares("request_count"));
        assert!(controls.automation("request_count").is_none());
        // The edges are inputs, not controls.
        for key in ["trigger", "reset"] {
            assert!(controls.get_control(key).is_err(), "{key}");
        }
        // The strings are never written from the audio thread: a scheduler
        // refuses string controls as it loads.
        for key in ["prompt", "status", "history"] {
            assert!(!controls.declares(key));
            assert!(matches!(
                controls.get_control(key),
                Ok(ControlValue::String(_))
            ));
        }
    }

    #[test]
    fn writes_before_the_module_runs_are_held_clamped() {
        let controls = controls();
        controls
            .set_control("cooldown", ControlValue::Number(-2.0))
            .unwrap();
        controls
            .set_control("enabled", ControlValue::Bool(false))
            .unwrap();
        assert_eq!(controls.get_control("cooldown"), Ok(0.0.into()));
        assert_eq!(controls.get_control("enabled"), Ok(false.into()));
        assert!(controls
            .set_control("cooldown", ControlValue::Number(f32::NAN))
            .is_err());
    }
}
