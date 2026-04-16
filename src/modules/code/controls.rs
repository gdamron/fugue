use std::sync::{Arc, Mutex};

use crate::{ControlMeta, ControlSurface, ControlValue};

/// Shared control surface for the orchestration-only `code` module.
///
/// Runtime hosts use these controls to communicate enabled state, status, and
/// last error back into the graph.
#[derive(Clone)]
pub struct CodeControls {
    shared: Arc<Mutex<CodeState>>,
}

#[derive(Clone, Debug)]
struct CodeState {
    enabled: bool,
    status: String,
    last_error: String,
    tick_hz: f32,
    script: String,
    entrypoint: String,
}

impl CodeControls {
    /// Creates a new control surface from static module config.
    pub fn new(enabled: bool, tick_hz: f32, script: String, entrypoint: Option<String>) -> Self {
        Self {
            shared: Arc::new(Mutex::new(CodeState {
                enabled,
                status: "idle".to_string(),
                last_error: String::new(),
                tick_hz,
                script,
                entrypoint: entrypoint.unwrap_or_else(|| "init".to_string()),
            })),
        }
    }

    /// Returns whether the script host should be active.
    pub fn enabled(&self) -> bool {
        self.shared.lock().unwrap().enabled
    }

    /// Enables or disables script execution.
    pub fn set_enabled(&self, enabled: bool) {
        self.shared.lock().unwrap().enabled = enabled;
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

    /// Returns the configured periodic tick rate in Hz.
    pub fn tick_hz(&self) -> f32 {
        self.shared.lock().unwrap().tick_hz
    }

    /// Updates the periodic tick rate in Hz, clamping it to zero or greater.
    pub fn set_tick_hz(&self, tick_hz: f32) {
        self.shared.lock().unwrap().tick_hz = tick_hz.max(0.0);
    }

    /// Returns the immutable script source captured from module config.
    pub fn script(&self) -> String {
        self.shared.lock().unwrap().script.clone()
    }

    /// Returns the configured startup entrypoint name.
    pub fn entrypoint(&self) -> String {
        self.shared.lock().unwrap().entrypoint.clone()
    }
}

impl ControlSurface for CodeControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::boolean(
                "enabled",
                "Enable or disable script execution",
                self.enabled(),
            ),
            ControlMeta::string("status", "Current orchestration runtime status")
                .with_default(self.status()),
            ControlMeta::string("last_error", "Last script runtime error")
                .with_default(self.last_error()),
            ControlMeta::number("tick_hz", "Periodic script tick frequency in Hz")
                .with_range(0.0, 1000.0)
                .with_default(self.tick_hz()),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "enabled" => Ok(self.enabled().into()),
            "status" => Ok(self.status().into()),
            "last_error" => Ok(self.last_error().into()),
            "tick_hz" => Ok(self.tick_hz().into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "enabled" => {
                self.set_enabled(value.as_bool()?);
                Ok(())
            }
            "status" => {
                self.set_status(value.as_string()?);
                Ok(())
            }
            "last_error" => {
                self.set_last_error(value.as_string()?);
                Ok(())
            }
            "tick_hz" => {
                self.set_tick_hz(value.as_number()?);
                Ok(())
            }
            _ => Err(format!("Unknown control: {}", key)),
        }
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
            .set_control("tick_hz", ControlValue::Number(8.0))
            .unwrap();
        assert_eq!(
            controls.get_control("tick_hz").unwrap(),
            ControlValue::Number(8.0)
        );
    }
}
