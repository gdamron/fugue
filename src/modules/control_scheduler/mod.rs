//! Control scheduler module for beat-synced automation of module controls.
//!
//! The scheduler is the interpretation layer's tool: it applies control
//! changes (mixer levels, tempo, pedal lanes) at musical positions instead of
//! by hand at performance time. Score-derived facts (dynamics, tempo) compile
//! *into* schedules; the score itself stays untouched.
//!
//! # Timing model
//!
//! The scheduler counts rising edges of its `clock` input — the same clock
//! gate that drives the sequencers — and fires every schedule entry whose
//! `at_step` is reached, on the exact frame of the edge. The first edge is
//! step 0, matching sequencer numbering. Step granularity is whatever clock
//! output is patched in (`beat` for beats, `beat_x4` for quarter beats, ...).
//!
//! A target whose module declares its controls is written through its
//! automation slots, which the graph applies just before the target
//! processes (see `crate::control_request::Automation`); a legacy target
//! through its shared control surface. Either way a change becomes visible
//! to a target when it processes its next block. The signal graph orders the scheduler before its targets (see
//! [`crate::Module::control_targets`]), so within a block the write is never
//! late; scheduling the same clock that drives the scheduler (tempo
//! automation) forms a cycle, which the graph processes sample-by-sample.
//!
//! What a target does with a change is its own: a mixer level, pan or
//! master starts its [`GAIN_RAMP_MS`](crate::modules::mixer::GAIN_RAMP_MS)
//! glide on the frame the change lands, so a cut is a 10 ms fade rather than
//! a click. Gating or panning that must be sample-exact goes through the
//! mixer's level and pan CV inputs.
//!
//! # Ramps
//!
//! An entry with `ramp_steps: N` leaves the control's current value at step
//! `at_step` and arrives exactly at `value` on the boundary of step
//! `at_step + N`,
//! interpolating linearly in between (hairpin-style automation). Boundary
//! values are exact: at every intermediate step boundary `at_step + k` the control
//! is exactly `from + (to - from) * k / N`, computed in `f64` and rounded once
//! to `f32`, whatever the measured step length. The interpolation never
//! overflows or leaves the range between the endpoints, so a ramp between any
//! two finite values writes only finite values.
//!
//! # Holds
//!
//! The scheduler is the automation tier. A user's, an agent's or a script's
//! write to a declared control it targets holds the control until the
//! scheduler's next entry on it: the ramp then running is abandoned at its
//! first write the hold drops, and the next entry takes the control back,
//! a ramp starting from the held value. An entry firing at the sample such
//! a write was due at loses to it, and the control stays held until the
//! entry after it.
//!
//! # Example Invention
//!
//! ```json
//! {
//!   "modules": [
//!     { "id": "clock", "type": "clock", "config": { "bpm": 120.0 } },
//!     { "id": "mixer", "type": "mixer", "config": { "channel_count": 2 } },
//!     {
//!       "id": "automation",
//!       "type": "control_scheduler",
//!       "config": {
//!         "schedule": [
//!           { "at_step": 0, "module": "mixer", "control": "level.0", "value": 0.2 },
//!           { "at_step": 8, "module": "mixer", "control": "level.0", "value": 1.0, "ramp_steps": 8 },
//!           { "at_step": 16, "module": "clock", "control": "bpm", "value": 90.0 }
//!         ]
//!       }
//!     }
//!   ],
//!   "connections": [
//!     { "from": "clock", "from_port": "beat", "to": "automation", "to_port": "clock" }
//!   ]
//! }
//! ```
//!
//! Schedules are plain JSON data, so they can be spliced in via `$asset`
//! references and replaced during playback through the `schedule` control.
//! A replacement is resolved on a control thread and handed to the scheduler
//! whole, as a payload request: the audio thread adopts it between blocks
//! without allocating, freeing or locking, and the schedule it replaces is
//! retired off the audio thread. Entries at or before the current step have
//! had their boundary; only later ones fire.
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
//!     "tempo_module": "clock",
//!     "tempo_scale": 1.0
//!   }
//! }
//! ```
//!
//! `tempo_module` (default `"clock"`) and `tempo_control` (default `"bpm"`)
//! name the control to write; `tempo_scale` (default `1.0`) is the invention's
//! interpretation knob, since the score records the notated quarter-note
//! tempo and the invention decides how its clock realizes it. Compiled tempo
//! entries merge with any explicit `schedule`. Patch the same clock output
//! that drives the sequencers into this module's `clock` input.

use std::sync::Arc;

use crate::control_request::{ControlCells, ControlIndex, ControlTable, Refusal};
use crate::payload::{Payload, Retired};
use crate::traits::ControlMeta;
use crate::Module;

pub use self::controls::ControlSchedulerControls;
pub use self::factory::ControlSchedulerFactory;
pub use self::schedule::{ScheduleEntry, ScheduleValue};

pub(crate) use self::controls::{Rebind, SurfaceDirectory, Written};
pub(crate) use self::factory::{
    attach_from_handle, attach_from_handle_resolving, name_unattached_from_handles,
};
pub(crate) use self::resolved::schedule_targets;

mod controls;
mod factory;
mod inputs;
mod outputs;
mod resolved;
mod schedule;

use controls::{SCHEDULE, TABLE};
use resolved::ResolvedSchedule;
use schedule::ResolvedEntry;

/// Module type id, shared with the runtime attachment points.
pub const CONTROL_SCHEDULER_TYPE_ID: &str = "control_scheduler";

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

/// The ramp value `from + (to - from) * progress` for `progress` in `[0, 1]`.
///
/// Computed in `f64`: `to - from` overflows `f32` when the endpoints are far
/// apart (`f32::MIN` to `f32::MAX`), which would write infinities, or NaN at
/// progress 0. Every step is monotonic in `progress`, and the result is kept
/// between the endpoints so rounding never overshoots `to` before the final
/// boundary writes it exactly. Uses `max`/`min` rather than `clamp`, which
/// panics on NaN bounds; a NaN `from` (never written by the scheduler) yields
/// `to`. Allocation-free: safe on the audio thread.
#[inline]
fn ramp_value(from: f32, to: f32, progress: f32) -> f32 {
    let (from_wide, to_wide) = (f64::from(from), f64::from(to));
    let value = (from_wide + (to_wide - from_wide) * f64::from(progress)) as f32;
    value.max(from.min(to)).min(from.max(to))
}

/// Applies scheduled control changes on clock-gate step boundaries.
///
/// # Inputs
///
/// - `clock` - Clock input (a rising edge advances the step counter)
/// - `reset` - Reset input (rising edge restarts the schedule from step 0)
///
/// # Outputs
///
/// - `step` - Current step index (-1.0 before the first gate edge)
pub struct ControlScheduler {
    #[allow(dead_code)] // Reserved (e.g. wall-clock schedule positions).
    sample_rate: u32,
    ctrl: ControlSchedulerControls,
    /// The declared `schedule` control's cells (see [`Module::declared`]).
    cells: Arc<ControlCells>,

    /// Adopted schedule, preloaded and sorted by `at` (audio-thread owned;
    /// handed over whole by the `schedule` payload, see [`resolved`]).
    entries: Vec<ResolvedEntry>,
    /// Next schedule entry to fire.
    cursor: usize,
    /// Current step index; -1 before the first gate edge.
    current_step: i64,

    /// Ramps in flight. Capacity is reserved with the schedule for every
    /// ramp entry in it, so pushes never allocate on the audio thread.
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
    /// Creates a new scheduler sharing the given controls. It plays nothing
    /// until its resolved schedule is handed to it, which binding its
    /// controls does once they are attached.
    pub fn new(sample_rate: u32, controls: ControlSchedulerControls) -> Self {
        Self {
            sample_rate,
            cells: controls.cells(),
            ctrl: controls,
            entries: Vec::new(),
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

    /// Plays `next` from here on, handing back the schedule it replaces in
    /// `next`'s box. Ramps in flight are abandoned, holding their last
    /// written value; entries at or before the current step have had their
    /// boundary, so only later ones fire. Swaps rather than moves, so it
    /// allocates, frees and locks nothing.
    fn adopt(&mut self, mut next: Box<ResolvedSchedule>) -> Box<ResolvedSchedule> {
        self.ctrl.set_adopted(next.id);
        std::mem::swap(&mut self.entries, &mut next.entries);
        std::mem::swap(&mut self.ramps, &mut next.ramps);
        self.ramps.clear();
        self.cursor = if self.current_step < 0 {
            0
        } else {
            let step = self.current_step as u64;
            self.entries.partition_point(|entry| entry.at_step <= step)
        };
        next
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
    /// boundary value `from + (to - from) * k / N` (see [`ramp_value`]; and
    /// exactly `to` on the final boundary).
    fn advance_ramps_on_edge(&mut self) {
        let entries = &self.entries;
        self.ramps.retain_mut(|ramp| {
            ramp.steps_done += 1;
            let entry = &entries[ramp.entry_idx];
            if ramp.steps_done >= ramp.steps_total {
                entry.write(ScheduleValue::Number(ramp.to));
                false
            } else {
                let progress = ramp.steps_done as f32 / ramp.steps_total as f32;
                // A ramp held off by a higher tier is abandoned.
                entry.write(ScheduleValue::Number(ramp_value(
                    ramp.from, ramp.to, progress,
                )))
            }
        });
    }

    /// Fires every schedule entry due at the current step, in schedule
    /// order, on frame `frame` of this block.
    fn fire_due_entries(&mut self, frame: usize) {
        debug_assert!(self.current_step >= 0);
        let step = self.current_step as u64;
        while self.cursor < self.entries.len() && self.entries[self.cursor].at_step <= step {
            let entry_idx = self.cursor;
            self.cursor += 1;
            self.cancel_conflicting_ramp(entry_idx);
            let entry = &self.entries[entry_idx];
            // The next event on its control takes it back from a hold.
            entry.begin(frame);
            if entry.ramp_steps == 0 {
                entry.write(entry.value);
                continue;
            }
            // Ramp: sample the control's current value as the start point.
            // Resolution validated the control is numeric; if the target
            // changed shape since, skip rather than misbehave.
            let Some(from) = entry.current_number() else {
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
        // A ramp held off by a higher tier is abandoned.
        self.ramps.retain(|ramp| {
            let progress = ((ramp.steps_done as f32 + frac) / ramp.steps_total as f32).min(1.0);
            let value = ramp_value(ramp.from, ramp.to, progress);
            entries[ramp.entry_idx].write(ScheduleValue::Number(value))
        });
    }

    /// Processes one sample.
    fn process_sample(&mut self, i: usize) {
        // Detect rising edges
        let gate_rising = self.inputs.clock(i) > 0.5 && self.last_gate_in <= 0.5;
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
            self.fire_due_entries(i);
        } else {
            self.update_ramps_between_edges();
        }

        // Count samples for step duration measurement
        self.samples_since_gate = self.samples_since_gate.saturating_add(1);

        self.outputs.set(i, self.current_step as f32);

        // Store for edge detection
        self.last_gate_in = self.inputs.clock(i);
        self.last_reset_in = self.inputs.reset(i);
    }
}

impl Module for ControlScheduler {
    fn name(&self) -> &str {
        "ControlScheduler"
    }

    fn process(&mut self, frames: usize) -> bool {
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

    #[allow(private_interfaces)]
    fn declared(&self) -> Option<(&ControlTable, &ControlCells)> {
        Some((&TABLE, &self.cells))
    }

    /// Takes a resolved schedule whole (see [`resolved`]); the one it
    /// replaces goes back to be retired off the audio thread.
    #[allow(private_interfaces)]
    fn apply_payload(
        &mut self,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<Retired, (Refusal, Payload)> {
        if control != SCHEDULE {
            return Err((Refusal::Unsupported, payload));
        }
        match payload.take_owned::<ResolvedSchedule>() {
            Ok(next) => Ok(self.adopt(next).into()),
            Err(payload) => Err((Refusal::Invalid, payload)),
        }
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
impl ControlScheduler {
    /// Hands the scheduler what its controls were written since the last
    /// call, as binding them to run does, leaving them taking writes for
    /// the next: a unit test's stand-in for a graph's request drain.
    pub(crate) fn settle(&mut self) {
        use crate::ControlSurface;
        let ctrl = self.ctrl.clone();
        ctrl.bind(
            crate::invention::declared::Route::Building(Vec::new()),
            self,
        );
    }
}

#[cfg(test)]
mod declared_tests;
#[cfg(test)]
mod payload_tests;
#[cfg(test)]
mod tests;
