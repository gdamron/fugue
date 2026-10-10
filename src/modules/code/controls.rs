//! The code module's controls: `enabled` and `tick_rate` are declared, and
//! the status strings and script stay control-side.
//!
//! The script host reads `enabled` and `tick_rate` back from their cells,
//! which only the thread running the module writes once it runs. The
//! strings sit behind a lock only control threads take: the module holds
//! the cells, never the lock.

use std::sync::{Arc, Mutex};

use crate::control_request::{
    Automation, ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind, RtValue,
};
use crate::invention::declared::{Declaration, DeclaredSurface, Route};
use crate::traits::{check_listed_control, ControlSurfaceMap};
use crate::{ControlMeta, ControlSurface, ControlValue, Module};

pub(super) const ENABLED: ControlIndex = ControlIndex(0);
pub(super) const TICK_RATE: ControlIndex = ControlIndex(1);

const DECLS: &[ControlDecl] = &[
    ControlDecl::new(
        "enabled",
        DeclKind::Bool,
        RtValue::Bool(true),
        "Enable or disable script execution",
    ),
    ControlDecl::new(
        "tick_rate",
        DeclKind::Number {
            min: 0.0,
            max: 1000.0,
        },
        RtValue::F32(0.0),
        "Periodic script tick frequency in Hz",
    )
    .unit("Hz")
    // Only negatives were ever clamped; above 1000 is held as written.
    .clamped(0.0, f32::MAX),
];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Shared control surface for the orchestration-only `code` module.
///
/// Runtime hosts use these controls to communicate enabled state, status, and
/// last error back into the graph.
#[derive(Clone)]
pub struct CodeControls {
    declared: Arc<DeclaredSurface>,
    cells: Arc<ControlCells>,
    shared: Arc<Mutex<CodeState>>,
}

#[derive(Clone, Debug)]
struct CodeState {
    status: String,
    last_error: String,
    script: String,
    entrypoint: String,
}

impl CodeControls {
    /// Creates a new control surface from static module config.
    pub fn new(enabled: bool, tick_rate: f32, script: String, entrypoint: Option<String>) -> Self {
        let cells = Arc::new(ControlCells::new(DECLS.iter().map(|decl| decl.default)));
        cells.publish(ENABLED, RtValue::Bool(enabled));
        cells.publish(TICK_RATE, RtValue::F32(tick_rate.max(0.0)));
        Self {
            declared: Arc::new(DeclaredSurface::new(TABLE.clone(), cells.clone())),
            cells,
            shared: Arc::new(Mutex::new(CodeState {
                status: "idle".to_string(),
                last_error: String::new(),
                script,
                entrypoint: entrypoint.unwrap_or_else(|| "init".to_string()),
            })),
        }
    }

    /// The cells the module holds and applies to.
    pub(super) fn cells(&self) -> Arc<ControlCells> {
        self.cells.clone()
    }

    /// Returns whether the script host should be active.
    pub fn enabled(&self) -> bool {
        matches!(self.cells.load(ENABLED), Some(RtValue::Bool(true)))
    }

    /// Enables or disables script execution, as `set_control` does: once
    /// the module runs, the write applies on the audio thread.
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        self.declared.set_control("enabled", enabled.into())
    }

    /// Returns the current runtime status string.
    pub fn status(&self) -> String {
        self.shared.lock().unwrap().status.clone()
    }

    /// Updates the current runtime status string.
    pub fn set_status(&self, status: impl Into<String>) {
        self.shared.lock().unwrap().status = status.into();
    }

    /// Returns the last runtime error reported by the script host.
    pub fn last_error(&self) -> String {
        self.shared.lock().unwrap().last_error.clone()
    }

    /// Stores the last runtime error reported by the script host.
    pub fn set_last_error(&self, last_error: impl Into<String>) {
        self.shared.lock().unwrap().last_error = last_error.into();
    }

    /// Returns the periodic tick rate in Hz.
    pub fn tick_rate(&self) -> f32 {
        match self.cells.load(TICK_RATE) {
            Some(RtValue::F32(rate)) => rate,
            _ => 0.0,
        }
    }

    /// Updates the periodic tick rate in Hz, clamped to zero or greater, as
    /// `set_control` does.
    pub fn set_tick_rate(&self, tick_rate: f32) -> Result<(), String> {
        self.declared.set_control("tick_rate", tick_rate.into())
    }

    /// Returns the immutable script source captured from module config.
    pub fn script(&self) -> String {
        self.shared.lock().unwrap().script.clone()
    }

    /// Returns the configured startup entrypoint name.
    pub fn entrypoint(&self) -> String {
        self.shared.lock().unwrap().entrypoint.clone()
    }

    /// Holds the strings' lock, for a test proving the audio path never
    /// takes it.
    #[cfg(test)]
    pub(crate) fn hold_state_lock(&self) -> std::sync::MutexGuard<'_, impl Sized> {
        self.shared.lock().unwrap()
    }
}

impl ControlSurface for CodeControls {
    fn controls(&self) -> Vec<ControlMeta> {
        let [enabled, tick_rate]: [ControlMeta; 2] = self
            .declared
            .controls()
            .try_into()
            .expect("the code module declares two controls");
        vec![
            enabled,
            ControlMeta::string("status", "Current orchestration runtime status")
                .with_default(self.status()),
            ControlMeta::string("last_error", "Last script runtime error")
                .with_default(self.last_error()),
            tick_rate,
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "status" => Ok(self.status().into()),
            "last_error" => Ok(self.last_error().into()),
            _ => self.declared.get_control(key),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "status" => self.set_status(value.as_string()?),
            "last_error" => self.set_last_error(value.as_string()?),
            _ => return self.declared.set_control(key, value),
        }
        Ok(())
    }

    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        surfaces: &ControlSurfaceMap,
    ) -> Result<(), String> {
        match self.declared.declares(key) {
            true => self.declared.validate_control(key, value, surfaces),
            false => check_listed_control(&self.controls(), key, value),
        }
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
    use super::CodeControls;
    use crate::{ControlSurface, ControlValue};

    #[test]
    fn code_controls_round_trip_values() {
        let controls = CodeControls::new(true, 4.0, "graph.status()".to_string(), None);
        assert_eq!(
            controls.get_control("enabled").unwrap(),
            ControlValue::Bool(true)
        );
        controls
            .set_control("tick_rate", ControlValue::Number(8.0))
            .unwrap();
        assert_eq!(
            controls.get_control("tick_rate").unwrap(),
            ControlValue::Number(8.0)
        );
        controls.set_tick_rate(-3.0).unwrap();
        assert_eq!(controls.tick_rate(), 0.0, "clamped");
        controls.set_tick_rate(2000.0).unwrap();
        assert_eq!(controls.tick_rate(), 2000.0, "held above the editor range");
        controls.set_enabled(false).unwrap();
        assert!(!controls.enabled());
        controls
            .set_control("status", ControlValue::String("running".into()))
            .unwrap();
        assert_eq!(controls.status(), "running");
    }

    #[test]
    fn controls_list_in_their_old_order_and_only_scalars_schedule() {
        let controls = CodeControls::new(true, 4.0, String::new(), None);
        let keys: Vec<String> = controls.controls().into_iter().map(|m| m.key).collect();
        assert_eq!(keys, ["enabled", "status", "last_error", "tick_rate"]);
        assert!(controls.automation("enabled").is_some());
        assert!(controls.automation("tick_rate").is_some());
        assert!(!controls.declares("status") && !controls.declares("last_error"));
    }
}
