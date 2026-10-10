//! Schedule data model for the ControlScheduler module.
//!
//! A schedule is an ordered list of control changes at musical positions.
//! Entries are declared as data (JSON, spliceable via `$asset`) and resolved
//! against the invention's control surfaces before playback, so the audio
//! thread applies them without lookups, locks, or allocation.

use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::control_request::Automation;
use crate::module_config::serde_whole;
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
/// `at_step` counts steps: rising edges of the scheduler's `clock` input,
/// with the first edge being step 0 — the same numbering the sequencers use.
/// The step granularity is whatever clock output the scheduler is patched to
/// (e.g. the clock's `beat`, or `beat_x4` for quarter beats). Positions in
/// beats or bars compile down to steps in whatever produces the schedule.
/// Step counts accept a JSON integer or a float that is exactly whole, and
/// an unknown field (such as a misspelt `ramp_steps`) is refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleEntry {
    /// Step boundary at which the change applies (clock rising-edge count,
    /// first edge = step 0).
    #[serde(deserialize_with = "serde_whole::whole")]
    pub at_step: u64,
    /// Target module id.
    pub module: String,
    /// Target control key on the module's control surface.
    pub control: String,
    /// Value to set (or to arrive at, when ramping).
    pub value: ScheduleValue,
    /// Optional linear ramp length in steps. The control leaves its current
    /// value at step `at_step` and arrives exactly at `value` on the step
    /// boundary `at_step + ramp_steps`. Numeric controls only.
    #[serde(
        default,
        deserialize_with = "serde_whole::optional_whole",
        skip_serializing_if = "Option::is_none"
    )]
    pub ramp_steps: Option<u64>,
}

/// Parses and validates a schedule from its JSON value form: an array of
/// entries, or the same array as JSON text. The text form is what the
/// `schedule` control takes, so a document whose control write was recorded
/// into the config builds with the schedule written.
pub(crate) fn parse_schedule(value: &serde_json::Value) -> Result<Vec<ScheduleEntry>, String> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    if let Some(json) = value.as_str() {
        return parse_schedule_json(json);
    }
    let entries: Vec<ScheduleEntry> = serde_json::from_value(value.clone())
        .map_err(|err| format!("invalid schedule: {}", err))?;
    validate_entries(&entries)?;
    Ok(entries)
}

/// One `{ at_step, bpm, ramp_steps? }` entry of a score tempo map, as spliced
/// in from a `fugue.score.v1` asset. Kept local (rather than importing the
/// score type) so the module layer stays independent of the score/invention
/// layer. Closed like the score's own point, so an old key is refused.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct TempoMapPoint {
    #[serde(deserialize_with = "serde_whole::whole")]
    at_step: u64,
    bpm: f32,
    /// Optional gradual glide to `bpm` over this many steps (ritardando /
    /// accelerando); absent = an instantaneous change.
    #[serde(default, deserialize_with = "serde_whole::optional_whole")]
    ramp_steps: Option<u64>,
}

/// Compiles a score tempo map into schedule entries that write a clock's tempo
/// control at each change's step boundary.
///
/// Each `{ at_step, bpm, ramp_steps? }` becomes `{ at_step, module, control,
/// value: bpm * tempo_scale, ramp_steps? }`. `tempo_scale` is the invention's
/// interpretation knob (default `1.0`): the score records the notated
/// quarter-note tempo, and the invention decides how its clock realizes it. An
/// entry's optional `ramp_steps` glides the tempo over that many steps (ritardando /
/// accelerando); without it the change is an instantaneous step at its
/// boundary.
pub(crate) fn compile_tempo_map(
    value: &serde_json::Value,
    module: &str,
    control: &str,
    tempo_scale: f32,
) -> Result<Vec<ScheduleEntry>, String> {
    if !(tempo_scale.is_finite() && tempo_scale > 0.0) {
        return Err(format!(
            "tempo_map tempo_scale must be a positive number, got {}",
            tempo_scale
        ));
    }
    let points: Vec<TempoMapPoint> = serde_json::from_value(value.clone())
        .map_err(|err| format!("invalid tempo_map: {}", err))?;
    let mut entries = Vec::with_capacity(points.len());
    for point in points {
        let value = point.bpm * tempo_scale;
        if !(value.is_finite() && value > 0.0) {
            return Err(format!(
                "tempo_map entry at step {}: scaled bpm ({}) must be positive",
                point.at_step, value
            ));
        }
        if let Some(ramp) = point.ramp_steps {
            if ramp < 1 {
                return Err(format!(
                    "tempo_map entry at step {}: ramp_steps must be at least 1 step",
                    point.at_step
                ));
            }
        }
        entries.push(ScheduleEntry {
            at_step: point.at_step,
            module: module.to_string(),
            control: control.to_string(),
            value: ScheduleValue::Number(value),
            ramp_steps: point.ramp_steps,
        });
    }
    Ok(entries)
}

/// Parses and validates a schedule from JSON text.
pub(crate) fn parse_schedule_json(json: &str) -> Result<Vec<ScheduleEntry>, String> {
    let entries: Vec<ScheduleEntry> =
        serde_json::from_str(json).map_err(|err| format!("invalid schedule: {}", err))?;
    validate_entries(&entries)?;
    Ok(entries)
}

/// Checks each entry's shape. A number must be finite: JSON cannot carry NaN,
/// but a value too large for an `f32` (such as `1e39`) parses as infinity,
/// and the audio thread writes entry values straight to module setters.
fn validate_entries(entries: &[ScheduleEntry]) -> Result<(), String> {
    for entry in entries {
        if let ScheduleValue::Number(number) = entry.value {
            if !number.is_finite() {
                return Err(format!(
                    "schedule entry at step {}: control '{}.{}' expects a finite number, got {}",
                    entry.at_step, entry.module, entry.control, number
                ));
            }
        }
        if let Some(ramp_steps) = entry.ramp_steps {
            if ramp_steps == 0 {
                return Err(format!(
                    "schedule entry at step {} for '{}.{}': ramp_steps must be at least 1",
                    entry.at_step, entry.module, entry.control
                ));
            }
            if !matches!(entry.value, ScheduleValue::Number(_)) {
                return Err(format!(
                    "schedule entry at step {} for '{}.{}': ramps require a numeric value",
                    entry.at_step, entry.module, entry.control
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
    pub(crate) at_step: u64,
    /// Target module id (kept for ordering-dependency discovery).
    pub(crate) module: String,
    pub(crate) control: String,
    pub(crate) value: ScheduleValue,
    /// Ramp length in steps; 0 means an immediate jump.
    pub(crate) ramp_steps: u64,
    pub(crate) surface: Arc<dyn ControlSurface + Send + Sync>,
    /// The target as automation writes it, when its module declares its
    /// controls: written without a lock, an allocation or a formatted
    /// error. `None` keeps the legacy setter.
    pub(crate) automation: Option<Automation>,
}

impl ResolvedEntry {
    /// Writes `value` to the target. Audio thread: allocation- and
    /// lock-free for a declared target; a legacy target's setter is its
    /// own (and its error, which may allocate, is dropped).
    #[inline]
    pub(crate) fn write(&self, value: ScheduleValue) {
        match (&self.automation, value) {
            (Some(target), ScheduleValue::Number(number)) => target.write_number(number),
            (Some(target), ScheduleValue::Bool(flag)) => target.write_bool(flag),
            (None, value) => {
                let _ = self
                    .surface
                    .set_control(&self.control, value.to_control_value());
            }
        }
    }

    /// The target's current number, where a ramp starts.
    #[inline]
    pub(crate) fn current_number(&self) -> Option<f32> {
        match &self.automation {
            Some(target) => target.current(),
            None => match self.surface.get_control(&self.control) {
                Ok(ControlValue::Number(number)) => Some(number),
                _ => None,
            },
        }
    }
}

/// Resolves schedule entries against the control surfaces of an invention.
///
/// Validates that every target module exists, is not the scheduler itself,
/// exposes the named control, and that the control's value type matches the
/// scheduled value. Returns entries stably sorted by `at_step` (ties keep schedule
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
                entry.at_step, own_id
            ));
        }
        let surface = surfaces.get(&entry.module).ok_or_else(|| {
            format!(
                "schedule entry at step {}: unknown module '{}' (or module has no controls)",
                entry.at_step, entry.module
            )
        })?;
        let current = surface.get_control(&entry.control).map_err(|err| {
            format!(
                "schedule entry at step {}: module '{}': {}",
                entry.at_step, entry.module, err
            )
        })?;
        match (&current, &entry.value) {
            (ControlValue::Number(_), ScheduleValue::Number(_)) => {}
            (ControlValue::Bool(_), ScheduleValue::Bool(_)) => {}
            (ControlValue::String(_), _) => {
                return Err(format!(
                    "schedule entry at step {}: control '{}.{}' is a string control; \
                     only numeric and boolean controls can be scheduled",
                    entry.at_step, entry.module, entry.control
                ));
            }
            _ => {
                return Err(format!(
                    "schedule entry at step {}: value type does not match control '{}.{}'",
                    entry.at_step, entry.module, entry.control
                ));
            }
        }
        // A declared control is written by automation or not at all: its
        // setter takes a lock, which the audio thread must never wait on.
        let automation = surface.automation(&entry.control);
        if automation.is_none() && surface.declares(&entry.control) {
            return Err(format!(
                "schedule entry at step {}: control '{}.{}' cannot be scheduled \
                 (an event or read-only control)",
                entry.at_step, entry.module, entry.control
            ));
        }
        resolved.push(ResolvedEntry {
            at_step: entry.at_step,
            module: entry.module.clone(),
            control: entry.control.clone(),
            value: entry.value,
            ramp_steps: entry.ramp_steps.unwrap_or(0),
            surface: surface.clone(),
            automation,
        });
    }
    resolved.sort_by_key(|entry| entry.at_step);
    Ok(resolved)
}
