//! Thread-safe controls for the ControlScheduler module.
//!
//! The schedule itself is a `Vec` behind a `Mutex`, gated by an atomic
//! version counter so the audio thread only locks (and re-adopts) when the
//! schedule actually changed — the same pattern as the cell sequencer's
//! sequence bank. Scalar state (`step`) lives in atomics.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::{ControlMeta, ControlSurface, ControlValue};

use super::schedule::{
    parse_schedule_json, resolve_schedule, ResolvedEntry, ScheduleEntry, SurfaceMap,
};

/// Shared directory of module control surfaces, owned by the runtime. The
/// scheduler holds a [`Weak`] reference so surfaces referenced by resolved
/// entries never keep the whole directory alive.
pub(crate) type SurfaceDirectory = Arc<Mutex<SurfaceMap>>;

/// Thread-safe controls for the ControlScheduler module.
///
/// Controls:
/// - `schedule` - The schedule as JSON (writable during playback)
/// - `step` - Read-only: current step index (-1 before the first gate)
#[derive(Clone)]
pub struct ControlSchedulerControls {
    shared: Arc<Shared>,
}

struct Shared {
    /// Bumped whenever the resolved schedule changes; the audio thread
    /// re-adopts on mismatch.
    version: AtomicU64,
    /// Current step index maintained by the audio thread (-1 before the
    /// first gate edge).
    step: AtomicI64,
    state: Mutex<ScheduleState>,
}

struct ScheduleState {
    /// The schedule as declared (config/JSON form).
    spec: Vec<ScheduleEntry>,
    /// The schedule resolved against control surfaces, preloaded for the
    /// audio thread.
    resolved: Vec<ResolvedEntry>,
    /// Runtime attachment: the scheduler's module id and the directory used
    /// to resolve schedule updates.
    attachment: Option<Attachment>,
}

struct Attachment {
    own_id: String,
    directory: Weak<Mutex<SurfaceMap>>,
}

impl ControlSchedulerControls {
    /// Creates controls holding an unresolved schedule. Resolution happens
    /// when the runtime attaches the scheduler (see [`Self::attach`]).
    pub(crate) fn new(spec: Vec<ScheduleEntry>) -> Self {
        Self {
            shared: Arc::new(Shared {
                version: AtomicU64::new(0),
                step: AtomicI64::new(-1),
                state: Mutex::new(ScheduleState {
                    spec,
                    resolved: Vec::new(),
                    attachment: None,
                }),
            }),
        }
    }

    /// Attaches the scheduler to its runtime: records its module id and the
    /// shared control-surface directory, then resolves the configured
    /// schedule. Returns an error (leaving the schedule unresolved) when any
    /// entry targets a missing module or control.
    pub(crate) fn attach(&self, own_id: &str, directory: &SurfaceDirectory) -> Result<(), String> {
        let spec = self.shared.state.lock().unwrap().spec.clone();
        let resolved = {
            let surfaces = directory.lock().unwrap();
            resolve_schedule(&spec, own_id, &surfaces)?
        };
        let mut state = self.shared.state.lock().unwrap();
        state.attachment = Some(Attachment {
            own_id: own_id.to_string(),
            directory: Arc::downgrade(directory),
        });
        state.resolved = resolved;
        drop(state);
        self.shared.version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Replaces the schedule from JSON text, re-resolving against the
    /// attached directory. On error the current schedule is left unchanged.
    pub fn set_schedule_json(&self, json: &str) -> Result<(), String> {
        let spec = parse_schedule_json(json)?;
        let (own_id, directory) = {
            let state = self.shared.state.lock().unwrap();
            let attachment = state
                .attachment
                .as_ref()
                .ok_or("control_scheduler is not attached to a runtime")?;
            (attachment.own_id.clone(), attachment.directory.clone())
        };
        let directory = directory
            .upgrade()
            .ok_or("control_scheduler runtime is gone")?;
        let resolved = {
            let surfaces = directory.lock().unwrap();
            resolve_schedule(&spec, &own_id, &surfaces)?
        };
        let mut state = self.shared.state.lock().unwrap();
        state.spec = spec;
        state.resolved = resolved;
        drop(state);
        self.shared.version.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Returns the schedule as JSON text.
    pub fn schedule_json(&self) -> String {
        serde_json::to_string(&self.shared.state.lock().unwrap().spec)
            .unwrap_or_else(|_| "[]".to_string())
    }

    /// Current resolved-schedule version (see [`Self::resolved_snapshot`]).
    pub(crate) fn version(&self) -> u64 {
        self.shared.version.load(Ordering::Acquire)
    }

    /// Clones the resolved schedule for adoption by the audio thread.
    /// Event-rate only: callers gate this behind [`Self::version`].
    pub(crate) fn resolved_snapshot(&self) -> Vec<ResolvedEntry> {
        self.shared.state.lock().unwrap().resolved.clone()
    }

    /// Module ids targeted by the resolved schedule, deduplicated. Used for
    /// process-order dependency discovery on topology changes.
    pub(crate) fn target_module_ids(&self) -> Vec<String> {
        let state = self.shared.state.lock().unwrap();
        let mut ids: Vec<String> = Vec::with_capacity(state.resolved.len());
        for entry in &state.resolved {
            if !ids.contains(&entry.module) {
                ids.push(entry.module.clone());
            }
        }
        ids
    }

    /// Current step index (-1 before the first gate edge).
    pub fn step(&self) -> i64 {
        self.shared.step.load(Ordering::Relaxed)
    }

    pub(crate) fn set_step(&self, step: i64) {
        self.shared.step.store(step, Ordering::Relaxed);
    }
}

impl ControlSurface for ControlSchedulerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::string(
                "schedule",
                "Schedule as JSON: [{at, module, control, value, ramp?}] \
                 (at = gate rising-edge count, first edge = step 0)",
            )
            .with_default(self.schedule_json()),
            ControlMeta::number(
                "step",
                "Read-only: current step index (-1 before first gate)",
            )
            .with_range(-1.0, f32::MAX)
            .with_default(self.step() as f32),
        ]
    }

    fn get_control(&self, key: &str) -> Result<ControlValue, String> {
        match key {
            "schedule" => Ok(self.schedule_json().into()),
            "step" => Ok((self.step() as f32).into()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "schedule" => self.set_schedule_json(value.as_string()?),
            "step" => Err("Control 'step' is read-only".to_string()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }
}
