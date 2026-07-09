//! Control scheduler module for beat-synced automation of module controls.
//!
//! The scheduler is the interpretation layer's tool: it applies control
//! changes (mixer levels, tempo, pedal lanes) at musical positions instead of
//! by hand at performance time. Score-derived facts (dynamics, tempo) compile
//! *into* schedules; the score itself stays untouched.
//!
//! # Timing model
//!
//! The scheduler counts rising edges of its `gate` input — the same clock
//! gate that drives the sequencers — and fires every schedule entry whose
//! `at` step is reached, on the exact frame of the edge. The first edge is
//! step 0, matching sequencer numbering. Step granularity is whatever gate
//! subdivision is patched in (`gate` for beats, `gate_x4` for 16ths, ...).
//!
//! Control writes go through the target module's shared atomic control
//! surface, so a change becomes visible to a target when it processes its
//! next block. The signal graph orders the scheduler before its targets (see
//! [`crate::Module::control_targets`]), so within a block the write is never
//! late; scheduling the same clock that drives the scheduler (tempo
//! automation) forms a cycle, which the graph processes sample-by-sample.
//!
//! # Ramps
//!
//! An entry with `ramp: N` leaves the control's current value at step `at`
//! and arrives exactly at `value` on the boundary of step `at + N`,
//! interpolating linearly in between (hairpin-style automation). Boundary
//! values are exact: at every intermediate step boundary `at + k` the control
//! is exactly `from + (to - from) * k / N`.
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
//!     { "id": "mixer", "type": "mixer", "config": { "channels": 2 } },
//!     {
//!       "id": "automation",
//!       "type": "control_scheduler",
//!       "config": {
//!         "schedule": [
//!           { "at": 0, "module": "mixer", "control": "level.0", "value": 0.2 },
//!           { "at": 8, "module": "mixer", "control": "level.0", "value": 1.0, "ramp": 8 },
//!           { "at": 16, "module": "clock", "control": "bpm", "value": 90.0 }
//!         ]
//!       }
//!     }
//!   ],
//!   "connections": [
//!     { "from": "clock", "from_port": "gate", "to": "automation", "to_port": "gate" }
//!   ]
//! }
//! ```
//!
//! Schedules are plain JSON data, so they can be spliced in via `$asset`
//! references and replaced during playback through the `schedule` control.
//!
//! # Tempo maps
//!
//! A scheduler can also compile a score's tempo map into tempo automation. Set
//! `tempo_map` to a `[{ at_step, bpm }]` array (typically spliced from a
//! `fugue.score.v1` asset) and the module appends one schedule entry per
//! change, writing the target clock's tempo at each step boundary:
//!
//! ```json
//! {
//!   "id": "tempo",
//!   "type": "control_scheduler",
//!   "config": {
//!     "tempo_map": { "$asset": "score", "path": "/tempo_map" },
//!     "tempo_target": "clock",
//!     "bpm_scale": 1.0
//!   }
//! }
//! ```
//!
//! `tempo_target` (default `"clock"`) and `tempo_control` (default `"bpm"`)
//! name the control to write; `bpm_scale` (default `1.0`) is the invention's
//! interpretation knob, since the score records the notated quarter-note
//! tempo and the invention decides how its clock realizes it. Compiled tempo
//! entries merge with any explicit `schedule`. Patch the same clock gate that
//! drives the sequencers into this module's `gate` input.

use std::sync::Arc;

use crate::factory::{GraphModule, ModuleBuildResult, ModuleFactory};
use crate::traits::ControlMeta;
use crate::{ControlValue, Module};

pub use self::controls::ControlSchedulerControls;
pub use self::schedule::{ScheduleEntry, ScheduleValue};

pub(crate) use self::controls::SurfaceDirectory;

mod controls;
mod inputs;
mod outputs;
mod schedule;

use schedule::ResolvedEntry;

/// Module type id, shared with the runtime attachment points.
pub const CONTROL_SCHEDULER_TYPE_ID: &str = "control_scheduler";

/// Factory for constructing ControlScheduler modules from configuration.
pub struct ControlSchedulerFactory;

impl ModuleFactory for ControlSchedulerFactory {
    fn type_id(&self) -> &'static str {
        CONTROL_SCHEDULER_TYPE_ID
    }

    fn build(
        &self,
        sample_rate: u32,
        config: &serde_json::Value,
    ) -> Result<ModuleBuildResult, Box<dyn std::error::Error>> {
        let mut spec =
            schedule::parse_schedule(config.get("schedule").unwrap_or(&serde_json::Value::Null))?;
        // A score tempo map (spliced in via `$asset`) compiles into schedule
        // entries that write a clock's tempo at each change's step boundary.
        if let Some(tempo_map) = config.get("tempo_map").filter(|value| !value.is_null()) {
            let module = config
                .get("tempo_target")
                .and_then(|value| value.as_str())
                .unwrap_or("clock");
            let control = config
                .get("tempo_control")
                .and_then(|value| value.as_str())
                .unwrap_or("bpm");
            let bpm_scale = config
                .get("bpm_scale")
                .and_then(|value| value.as_f64())
                .unwrap_or(1.0) as f32;
            spec.extend(schedule::compile_tempo_map(
                tempo_map, module, control, bpm_scale,
            )?);
        }
        let controls = ControlSchedulerControls::new(spec);
        let module = ControlScheduler::new(sample_rate, controls.clone());

        Ok(ModuleBuildResult {
            module: GraphModule::Module(Box::new(module)),
            handles: vec![(
                "controls".to_string(),
                Arc::new(controls.clone()) as Arc<dyn std::any::Any + Send + Sync>,
            )],
            control_surface: Some(Arc::new(controls)),
            sink: None,
        })
    }
}

/// Attaches a just-built scheduler to the runtime's control-surface
/// directory via its type-erased `controls` handle. Shared by every path
/// that can introduce a scheduler (builder, live add, swap).
pub(crate) fn attach_from_handle(
    module_id: &str,
    handle: Option<&Arc<dyn std::any::Any + Send + Sync>>,
    directory: &SurfaceDirectory,
) -> Result<(), String> {
    let controls = handle
        .and_then(|handle| handle.downcast_ref::<ControlSchedulerControls>())
        .ok_or_else(|| {
            format!(
                "control_scheduler '{}' is missing its controls handle",
                module_id
            )
        })?;
    controls.attach(module_id, directory)
}

/// A ramp in flight: linear interpolation of one numeric control across step
/// boundaries. Holds an index into the adopted schedule, never owned data, so
/// activating and completing ramps is allocation-free.
struct ActiveRamp {
    /// Index into the adopted `entries`.
    entry_idx: usize,
    /// Control value sampled when the ramp fired.
    from: f32,
    /// Scheduled arrival value.
    to: f32,
    /// Total ramp length in steps (>= 1).
    steps_total: u64,
    /// Step boundaries crossed since the ramp fired.
    steps_done: u64,
}

/// Applies scheduled control changes on clock-gate step boundaries.
///
/// # Inputs
///
/// - `gate` - Clock gate input (rising edge advances the step counter)
/// - `reset` - Reset input (rising edge restarts the schedule from step 0)
///
/// # Outputs
///
/// - `step` - Current step index (-1.0 before the first gate edge)
pub struct ControlScheduler {
    #[allow(dead_code)] // Reserved (e.g. wall-clock schedule positions).
    sample_rate: u32,
    ctrl: ControlSchedulerControls,

    /// Adopted schedule, preloaded and sorted by `at` (audio-thread owned;
    /// re-adopted when the controls' version changes).
    entries: Vec<ResolvedEntry>,
    entries_version: u64,
    /// Next schedule entry to fire.
    cursor: usize,
    /// Current step index; -1 before the first gate edge.
    current_step: i64,

    /// Ramps in flight. Capacity is reserved at adoption for every ramp entry
    /// in the schedule, so pushes never allocate on the audio thread.
    ramps: Vec<ActiveRamp>,

    /// Duration of one step in samples, measured between gate edges.
    step_duration_samples: u32,
    /// Samples since the last gate edge (0 on the edge frame).
    samples_since_gate: u32,
    first_gate_received: bool,

    last_gate_in: f32,
    last_reset_in: f32,

    inputs: inputs::ControlSchedulerInputs,
    outputs: outputs::ControlSchedulerOutputs,
}

impl ControlScheduler {
    /// Creates a new scheduler sharing the given controls.
    pub fn new(sample_rate: u32, controls: ControlSchedulerControls) -> Self {
        Self {
            sample_rate,
            ctrl: controls,
            entries: Vec::new(),
            entries_version: 0,
            cursor: 0,
            current_step: -1,
            ramps: Vec::new(),
            step_duration_samples: sample_rate / 2, // Default ~120 BPM
            samples_since_gate: 0,
            first_gate_received: false,
            last_gate_in: 0.0,
            last_reset_in: 0.0,
            inputs: inputs::ControlSchedulerInputs::new(),
            outputs: outputs::ControlSchedulerOutputs::new(),
        }
    }

    /// Returns a reference to the scheduler controls.
    pub fn controls(&self) -> &ControlSchedulerControls {
        &self.ctrl
    }

    /// Adopts a changed schedule from the controls (event-rate: gated by the
    /// version counter, so playback never locks or allocates steady-state).
    fn adopt_schedule_if_changed(&mut self) {
        let version = self.ctrl.version();
        if version == self.entries_version {
            return;
        }
        self.entries = self.ctrl.resolved_snapshot();
        self.entries_version = version;
        self.ramps.clear();
        let ramp_count = self
            .entries
            .iter()
            .filter(|entry| entry.ramp_steps > 0)
            .count();
        self.ramps = Vec::with_capacity(ramp_count);
        // Entries at or before the current step have had their boundary
        // already; only future boundaries fire.
        self.cursor = if self.current_step < 0 {
            0
        } else {
            let step = self.current_step as u64;
            self.entries.partition_point(|entry| entry.at <= step)
        };
    }

    /// Restarts the schedule: the next gate edge is step 0 again and every
    /// entry re-arms. Ramps in flight are cancelled, holding their last
    /// written value.
    fn reset(&mut self) {
        self.current_step = -1;
        self.cursor = 0;
        self.ramps.clear();
        self.first_gate_received = false;
        self.samples_since_gate = 0;
    }

    /// Cancels any ramp in flight that writes the same control as `entry_idx`
    /// (a newer scheduled change supersedes it).
    fn cancel_conflicting_ramp(&mut self, entry_idx: usize) {
        let entries = &self.entries;
        let target = &entries[entry_idx];
        self.ramps.retain(|ramp| {
            let ramp_entry = &entries[ramp.entry_idx];
            ramp_entry.module != target.module || ramp_entry.control != target.control
        });
    }

    /// Advances ramps in flight across a step boundary, writing the exact
    /// boundary value `from + (to - from) * k / N` (and exactly `to` on the
    /// final boundary).
    fn advance_ramps_on_edge(&mut self) {
        let entries = &self.entries;
        self.ramps.retain_mut(|ramp| {
            ramp.steps_done += 1;
            let entry = &entries[ramp.entry_idx];
            if ramp.steps_done >= ramp.steps_total {
                let _ = entry
                    .surface
                    .set_control(&entry.control, ControlValue::Number(ramp.to));
                false
            } else {
                let progress = ramp.steps_done as f32 / ramp.steps_total as f32;
                let value = ramp.from + (ramp.to - ramp.from) * progress;
                let _ = entry
                    .surface
                    .set_control(&entry.control, ControlValue::Number(value));
                true
            }
        });
    }

    /// Fires every schedule entry due at the current step, in schedule order.
    fn fire_due_entries(&mut self) {
        debug_assert!(self.current_step >= 0);
        let step = self.current_step as u64;
        while self.cursor < self.entries.len() && self.entries[self.cursor].at <= step {
            let entry_idx = self.cursor;
            self.cursor += 1;
            self.cancel_conflicting_ramp(entry_idx);
            let entry = &self.entries[entry_idx];
            if entry.ramp_steps == 0 {
                let _ = entry
                    .surface
                    .set_control(&entry.control, entry.value.to_control_value());
                continue;
            }
            // Ramp: sample the control's current value as the start point.
            // Resolution validated the control is numeric; if the target
            // changed shape since, skip rather than misbehave.
            let Ok(ControlValue::Number(from)) = entry.surface.get_control(&entry.control) else {
                continue;
            };
            let ScheduleValue::Number(to) = entry.value else {
                continue;
            };
            self.ramps.push(ActiveRamp {
                entry_idx,
                from,
                to,
                steps_total: entry.ramp_steps,
                steps_done: 0,
            });
        }
    }

    /// Writes interpolated values for ramps in flight between step
    /// boundaries. Progress within the step is estimated from the measured
    /// step duration and clamped so the next boundary value is never
    /// overshot before its edge arrives.
    fn update_ramps_between_edges(&mut self) {
        if self.ramps.is_empty() {
            return;
        }
        let duration = self.step_duration_samples.max(1) as f32;
        let frac = (self.samples_since_gate as f32 / duration).min(1.0);
        let entries = &self.entries;
        for ramp in &self.ramps {
            let progress = ((ramp.steps_done as f32 + frac) / ramp.steps_total as f32).min(1.0);
            let value = ramp.from + (ramp.to - ramp.from) * progress;
            let entry = &entries[ramp.entry_idx];
            let _ = entry
                .surface
                .set_control(&entry.control, ControlValue::Number(value));
        }
    }

    /// Processes one sample.
    fn process_sample(&mut self, i: usize) {
        // Detect rising edges
        let gate_rising = self.inputs.gate(i) > 0.5 && self.last_gate_in <= 0.5;
        let reset_rising = self.inputs.reset(i) > 0.5 && self.last_reset_in <= 0.5;

        // Handle reset (takes priority)
        if reset_rising {
            self.reset();
        }

        if gate_rising {
            // Measure step duration from the previous edge
            if self.first_gate_received && self.samples_since_gate > 0 {
                self.step_duration_samples = self.samples_since_gate;
            }
            self.first_gate_received = true;
            self.samples_since_gate = 0;

            self.current_step += 1;
            self.advance_ramps_on_edge();
            self.fire_due_entries();
        } else {
            self.update_ramps_between_edges();
        }

        // Count samples for step duration measurement
        self.samples_since_gate = self.samples_since_gate.saturating_add(1);

        self.outputs.set(i, self.current_step as f32);

        // Store for edge detection
        self.last_gate_in = self.inputs.gate(i);
        self.last_reset_in = self.inputs.reset(i);
    }
}

impl Module for ControlScheduler {
    fn name(&self) -> &str {
        "ControlScheduler"
    }

    fn process(&mut self, frames: usize) -> bool {
        self.adopt_schedule_if_changed();
        for i in 0..frames {
            self.process_sample(i);
        }
        // Mirror the step counter into the controls at block rate.
        self.ctrl.set_step(self.current_step);
        true
    }

    fn inputs(&self) -> &[&str] {
        &inputs::INPUTS
    }

    fn outputs(&self) -> &[&str] {
        &outputs::OUTPUTS
    }

    fn set_input(&mut self, port: &str, value: f32) -> Result<(), String> {
        self.inputs.set(port, value)
    }

    fn input_block_mut(&mut self, index: usize) -> &mut [f32] {
        self.inputs.block_mut(index)
    }

    fn output_block(&self, index: usize) -> &[f32] {
        self.outputs.block(index)
    }

    fn get_output(&self, port: &str) -> Result<f32, String> {
        self.outputs.get(port)
    }

    fn control_targets(&self) -> Vec<String> {
        self.ctrl.target_module_ids()
    }

    fn controls(&self) -> Vec<ControlMeta> {
        use crate::ControlSurface;
        self.ctrl.controls()
    }

    fn get_control(&self, key: &str) -> Result<f32, String> {
        match key {
            "step" => Ok(self.current_step as f32),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    fn set_control(&mut self, key: &str, _value: f32) -> Result<(), String> {
        Err(format!("Control '{}' is not numeric-settable", key))
    }
}

#[cfg(test)]
mod tests;
