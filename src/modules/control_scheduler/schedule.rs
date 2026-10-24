//! Schedule data model for the ControlScheduler module.
//!
//! A schedule is an ordered list of control changes at musical positions.
//! Entries are declared as data (JSON, spliceable via `$asset`) and resolved
//! against the invention's control surfaces before playback, so the audio
//! thread applies them without lookups, locks, or allocation.

use std::sync::Arc;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::control_request::{EventCounter, TableEntry, TableTarget, TableValue};
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

impl From<ScheduleValue> for TableValue {
    fn from(value: ScheduleValue) -> Self {
        match value {
            ScheduleValue::Number(value) => Self::Number(value),
            ScheduleValue::Bool(value) => Self::Bool(value),
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
/// preloaded for the audio thread: the table entry it plays (see
/// [`crate::control_request::BeatTable`]), and the control it names.
#[derive(Clone)]
pub(crate) struct ResolvedEntry {
    /// Target module id (kept for ordering-dependency discovery).
    pub(crate) module: String,
    pub(crate) control: String,
    /// Its step, ramp, value, and target: the control's automation slot
    /// when its module declares its controls (written without a lock, an
    /// allocation or a formatted error), else its legacy setter.
    pub(crate) entry: TableEntry,
}

impl ResolvedEntry {
    pub(crate) fn at_step(&self) -> u64 {
        self.entry.step
    }

    pub(crate) fn ramp_steps(&self) -> u64 {
        self.entry.ramp_steps
    }

    /// Writes `value` to the target, returning false when a higher tier
    /// holds it and the write was dropped (see `TableTarget::write`).
    #[inline]
    pub(crate) fn write(&self, value: TableValue) -> bool {
        self.entry.target.write(value)
    }

    /// Automation's next event on the target, on frame `frame` of the
    /// scheduler's processing: frees it from a higher tier's hold (see
    /// `Automation::begin`). A legacy target has no tiers.
    #[inline]
    pub(crate) fn begin(&self, frame: usize) {
        self.entry.target.begin(frame);
    }

    /// The target's current number, where a ramp starts.
    #[inline]
    pub(crate) fn current_number(&self) -> Option<f32> {
        self.entry.target.current()
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
    resolve_schedule_counting(entries, own_id, surfaces, &Arc::new(EventCounter::new()))
}

/// [`resolve_schedule`], with the legacy targets counting the writes and
/// reads their surfaces refuse in `refused`.
pub(crate) fn resolve_schedule_counting(
    entries: &[ScheduleEntry],
    own_id: &str,
    surfaces: &SurfaceMap,
    refused: &Arc<EventCounter>,
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
                 (a read-only or payload control)",
                entry.at_step, entry.module, entry.control
            ));
        }
        // A ramp writes on every block it crosses, so it would fire an
        // event again and again: an event is jumped to, once.
        if entry.ramp_steps.is_some() && automation.as_ref().is_some_and(|a| a.event) {
            return Err(format!(
                "schedule entry at step {}: control '{}.{}' is a trigger; \
                 it fires at a step and cannot be ramped",
                entry.at_step, entry.module, entry.control
            ));
        }
        let target = match automation {
            Some(automation) => TableTarget::Declared(automation),
            None => TableTarget::legacy(surface.clone(), entry.control.clone(), refused.clone()),
        };
        resolved.push(ResolvedEntry {
            module: entry.module.clone(),
            control: entry.control.clone(),
            entry: TableEntry {
                step: entry.at_step,
                ramp_steps: entry.ramp_steps.unwrap_or(0),
                value: entry.value.into(),
                target,
            },
        });
    }
    resolved.sort_by_key(ResolvedEntry::at_step);
    Ok(resolved)
}
