//! Schedule data model for the ControlScheduler module.
//!
//! A schedule is an ordered list of control changes at musical positions.
//! Entries are declared as data (JSON, spliceable via `$asset`) and resolved
//! against the invention's control surfaces before playback, so the audio
//! thread applies them without lookups, locks, or allocation.

use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::{ControlSurface, ControlValue};

/// Shared control surface map used to resolve schedule targets.
pub(crate) type SurfaceMap = IndexMap<String, Arc<dyn ControlSurface + Send + Sync>>;

/// A scheduled control value. Only numbers and booleans are supported so the
/// audio thread can apply changes without allocating (string control values
/// would need a heap clone per write).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleValue {
    Number(f32),
    Bool(bool),
}

impl ScheduleValue {
    /// Converts to a [`ControlValue`] without allocating.
    #[inline]
    pub(crate) fn to_control_value(self) -> ControlValue {
        match self {
            Self::Number(value) => ControlValue::Number(value),
            Self::Bool(value) => ControlValue::Bool(value),
        }
    }
}

/// One scheduled control change.
///
/// `at` counts steps: rising edges of the scheduler's `gate` input, with the
/// first edge being step 0 — the same numbering the sequencers use. The step
/// granularity is whatever clock gate the scheduler is patched to (e.g. the
/// clock's `gate` for beats, `gate_x4` for 16ths). Positions in beats or
/// measures compile down to steps in whatever produces the schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleEntry {
    /// Step boundary at which the change applies (gate rising-edge count,
    /// first edge = step 0).
    pub at: u64,
    /// Target module id.
    pub module: String,
    /// Target control key on the module's control surface.
    pub control: String,
    /// Value to set (or to arrive at, when ramping).
    pub value: ScheduleValue,
    /// Optional linear ramp length in steps. The control leaves its current
    /// value at step `at` and arrives exactly at `value` on the step boundary
    /// `at + ramp`. Numeric controls only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ramp: Option<u64>,
}

/// Parses and validates a schedule from its JSON value form.
pub(crate) fn parse_schedule(value: &serde_json::Value) -> Result<Vec<ScheduleEntry>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let entries: Vec<ScheduleEntry> = serde_json::from_value(value.clone())
        .map_err(|err| format!("invalid schedule: {}", err))?;
    validate_entries(&entries)?;
    Ok(entries)
}

/// Parses and validates a schedule from JSON text.
pub(crate) fn parse_schedule_json(json: &str) -> Result<Vec<ScheduleEntry>, String> {
    let entries: Vec<ScheduleEntry> =
        serde_json::from_str(json).map_err(|err| format!("invalid schedule: {}", err))?;
    validate_entries(&entries)?;
    Ok(entries)
}

fn validate_entries(entries: &[ScheduleEntry]) -> Result<(), String> {
    for entry in entries {
        if let Some(ramp) = entry.ramp {
            if ramp == 0 {
                return Err(format!(
                    "schedule entry at step {} for '{}.{}': ramp must be at least 1 step",
                    entry.at, entry.module, entry.control
                ));
            }
            if !matches!(entry.value, ScheduleValue::Number(_)) {
                return Err(format!(
                    "schedule entry at step {} for '{}.{}': ramps require a numeric value",
                    entry.at, entry.module, entry.control
                ));
            }
        }
    }
    Ok(())
}

/// A schedule entry resolved against the invention's control surfaces,
/// preloaded for the audio thread.
#[derive(Clone)]
pub(crate) struct ResolvedEntry {
    pub(crate) at: u64,
    /// Target module id (kept for ordering-dependency discovery).
    pub(crate) module: String,
    pub(crate) control: String,
    pub(crate) value: ScheduleValue,
    /// Ramp length in steps; 0 means an immediate jump.
    pub(crate) ramp_steps: u64,
    pub(crate) surface: Arc<dyn ControlSurface + Send + Sync>,
}

/// Resolves schedule entries against the control surfaces of an invention.
///
/// Validates that every target module exists, is not the scheduler itself,
/// exposes the named control, and that the control's value type matches the
/// scheduled value. Returns entries stably sorted by `at` (ties keep schedule
/// order).
pub(crate) fn resolve_schedule(
    entries: &[ScheduleEntry],
    own_id: &str,
    surfaces: &SurfaceMap,
) -> Result<Vec<ResolvedEntry>, String> {
    let mut resolved = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.module == own_id {
            return Err(format!(
                "schedule entry at step {}: a control_scheduler cannot target itself ('{}')",
                entry.at, own_id
            ));
        }
        let surface = surfaces.get(&entry.module).ok_or_else(|| {
            format!(
                "schedule entry at step {}: unknown module '{}' (or module has no controls)",
                entry.at, entry.module
            )
        })?;
        let current = surface.get_control(&entry.control).map_err(|err| {
            format!(
                "schedule entry at step {}: module '{}': {}",
                entry.at, entry.module, err
            )
        })?;
        match (&current, &entry.value) {
            (ControlValue::Number(_), ScheduleValue::Number(_)) => {}
            (ControlValue::Bool(_), ScheduleValue::Bool(_)) => {}
            (ControlValue::String(_), _) => {
                return Err(format!(
                    "schedule entry at step {}: control '{}.{}' is a string control; \
                     only numeric and boolean controls can be scheduled",
                    entry.at, entry.module, entry.control
                ));
            }
            _ => {
                return Err(format!(
                    "schedule entry at step {}: value type does not match control '{}.{}'",
                    entry.at, entry.module, entry.control
                ));
            }
        }
        resolved.push(ResolvedEntry {
            at: entry.at,
            module: entry.module.clone(),
            control: entry.control.clone(),
            value: entry.value,
            ramp_steps: entry.ramp.unwrap_or(0),
            surface: surface.clone(),
        });
    }
    resolved.sort_by_key(|entry| entry.at);
    Ok(resolved)
}
