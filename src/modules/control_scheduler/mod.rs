//! Control scheduler module for beat-synced automation of module controls.
//!
//! The scheduler is the interpretation layer's tool: it applies control
//! changes (mixer levels, tempo, pedal lanes) at musical positions instead of
//! by hand at performance time. Score-derived facts (dynamics, tempo) compile
//! *into* schedules; the score itself stays untouched.
//!
//! # Timing model
//!
//! A step is one gate of the clock output patched into `clock` — the same
//! clock gate that drives the sequencers: `beat` for beats, `beat_x4` for
//! quarter beats, and so on. Every schedule entry fires on the exact sample
//! its step falls on, as a request timed in beats applies.
//!
//! In an invention's top level the graph's request drain plays the
//! schedule in place, on the clock's position (see
//! `crate::control_request::BeatTable`). There `clock` takes one clock's
//! gate output, or nothing: a scheduler on any other signal, or on several,
//! is refused at load and by an edit. Then step 0 falls on the clock's first
//! gate past where it stood when the drain first saw the schedule (beat 0
//! for a clock not yet started), whatever the block boundaries, and a
//! tempo an entry writes moves the next steps at once. When the clock
//! resets, the schedule plays again from step 0 on the clock's new beat 0;
//! the `reset` input is not followed there. The `step` output follows the
//! clock's position. When two schedulers write one control at one sample,
//! the later module's write lands, unless the earlier one's entry only fell
//! due there because the later one wrote its clock's tempo at that sample:
//! it lands after the write that moved it. (A scheduler playing its
//! schedule itself, below, writes as it processes, after every schedule
//! the drain plays.) Module order is the order of the
//! invention's `modules` at load: a module rebuilt in place keeps its
//! place, and one added later, live or by a reload, goes after every
//! other. Reordering `modules` in a reloaded document doesn't reorder the
//! running modules, so the order is load order plus edit history.
//!
//! When the drain stops playing a schedule (its clock unplugged), the
//! scheduler plays on by itself from the step the clock reached, counting
//! gates as inside a development; a ramp then under way stops where it
//! stands. Plugging a clock in, moving the cable to another clock output,
//! or replacing the clock starts the schedule over: step 0 falls on that
//! clock's next gate, and every entry plays again.
//!
//! Inside a development, the scheduler plays the schedule itself: it counts
//! rising edges of its `clock` input, the first being step 0, and a rising
//! edge on `reset` restarts the schedule from step 0.
//!
//! A target whose module declares its controls is written through its
//! automation slots, which the graph applies just before the target
//! processes (see `crate::control_request::Automation`); a legacy target
//! through its shared control surface, with no tiers.
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
//! `at_step` and arrives exactly at `value` on the sample of step
//! `at_step + N`, interpolating linearly in between (hairpin-style
//! automation). Played by the drain, the ramp follows the clock's position
//! and is written at the start of each processing segment (a block, or
//! part of one); played by the scheduler itself, every sample, from the
//! measured step length, with every step boundary `at_step + k` exactly
//! `from + (to - from) * k / N`. Values are computed in `f64` and rounded
//! once to `f32`, never overflow, and never leave the range between the
//! endpoints, so a ramp between any two finite values writes only finite
//! values.
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

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::control_request::{
    BeatTable, ControlCells, ControlIndex, ControlTable, Refusal, TableEntry, TableValue,
};
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
use resolved::{same_plan, ResolvedSchedule};
use schedule::ResolvedEntry;

/// Module type id, shared with the runtime attachment points.
pub const CONTROL_SCHEDULER_TYPE_ID: &str = "control_scheduler";

/// The next scheduler's [`BeatTable::id`]: never reused.
static NEXT_TABLE: AtomicUsize = AtomicUsize::new(1);

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
/// - `clock` - Clock input (a gate of the clock output patched here is a
///   step; at the top level, only a clock's gate output is accepted)
/// - `reset` - Reset input (inside a development, a rising edge restarts
///   the schedule from step 0; at the top level the clock's reset does)
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

    /// Its schedule as the graph's drain plays it (see [`BeatTable`]): its
    /// id, the plan it plays (changed by every adoption but a retarget),
    /// whether the drain plays it, and the step the drain last told it.
    table: usize,
    plan: u64,
    played: AtomicBool,
    told_step: AtomicI64,
    /// Whether the last block followed the drain: the first it plays on
    /// its own takes the step the drain told since.
    followed: bool,

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
            table: NEXT_TABLE.fetch_add(1, Ordering::Relaxed),
            plan: 0,
            played: AtomicBool::new(false),
            told_step: AtomicI64::new(-1),
            followed: false,
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
    /// boundary, so only later ones fire. A retarget of the schedule it
    /// plays only swaps the targets in, and ramps carry on. Swaps rather
    /// than moves, so it allocates, frees and locks nothing.
    fn adopt(&mut self, mut next: Box<ResolvedSchedule>) -> Box<ResolvedSchedule> {
        self.ctrl.set_adopted(next.id);
        if next.retarget && same_plan(&self.entries, &next.entries) {
            // The same entries, re-resolved: only their targets change, so
            // the step and every ramp in flight (by entry index) carry on.
            std::mem::swap(&mut self.entries, &mut next.entries);
            return next;
        }
        std::mem::swap(&mut self.entries, &mut next.entries);
        std::mem::swap(&mut self.ramps, &mut next.ramps);
        self.plan += 1;
        self.ramps.clear();
        self.skip_played();
        next
    }

    /// Moves the cursor past every entry at or before the current step:
    /// those have had their boundary.
    fn skip_played(&mut self) {
        self.cursor = if self.current_step < 0 {
            0
        } else {
            let step = self.current_step as u64;
            self.entries
                .partition_point(|entry| entry.at_step() <= step)
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
    /// boundary value `from + (to - from) * k / N` (see [`ramp_value`]; and
    /// exactly `to` on the final boundary).
    fn advance_ramps_on_edge(&mut self) {
        let entries = &self.entries;
        self.ramps.retain_mut(|ramp| {
            ramp.steps_done += 1;
            let entry = &entries[ramp.entry_idx];
            if ramp.steps_done >= ramp.steps_total {
                entry.write(TableValue::Number(ramp.to));
                false
            } else {
                let progress = ramp.steps_done as f32 / ramp.steps_total as f32;
                // A ramp held off by a higher tier is abandoned.
                entry.write(TableValue::Number(ramp_value(ramp.from, ramp.to, progress)))
            }
        });
    }

    /// Fires every schedule entry due at the current step, in schedule
    /// order, on frame `frame` of this block.
    fn fire_due_entries(&mut self, frame: usize) {
        debug_assert!(self.current_step >= 0);
        let step = self.current_step as u64;
        while self.cursor < self.entries.len() && self.entries[self.cursor].at_step() <= step {
            let entry_idx = self.cursor;
            self.cursor += 1;
            self.cancel_conflicting_ramp(entry_idx);
            let entry = &self.entries[entry_idx];
            // The next event on its control takes it back from a hold.
            entry.begin(frame);
            if entry.ramp_steps() == 0 {
                entry.write(entry.entry.value);
                continue;
            }
            // Ramp: sample the control's current value as the start point.
            // Resolution validated the control is numeric; if the target
            // changed shape since, skip rather than misbehave.
            let Some(from) = entry.current_number() else {
                continue;
            };
            let TableValue::Number(to) = entry.entry.value else {
                continue;
            };
            self.ramps.push(ActiveRamp {
                entry_idx,
                from,
                to,
                steps_total: entry.ramp_steps(),
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
            entries[ramp.entry_idx].write(TableValue::Number(value))
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
        if self.played.load(Ordering::Relaxed) {
            self.followed = true;
            self.follow_drain(frames);
        } else {
            if std::mem::take(&mut self.followed) {
                // The drain may have played a step since the last block.
                self.take_told();
            }
            for i in 0..frames {
                self.process_sample(i);
            }
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
    fn beat_table(&self) -> Option<&dyn BeatTable> {
        Some(self)
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

impl ControlScheduler {
    /// A block the drain plays the schedule in: the step is the one the
    /// drain told, constant through the block (the drain splits blocks on
    /// each step's sample). The gate and reset inputs are only followed,
    /// so playing on its own again sees no stale edge.
    fn follow_drain(&mut self, frames: usize) {
        self.take_told();
        for i in 0..frames {
            self.outputs.set(i, self.current_step as f32);
        }
        if let Some(last) = frames.checked_sub(1) {
            self.last_gate_in = self.inputs.clock(last);
            self.last_reset_in = self.inputs.reset(last);
        }
    }

    /// Takes the step the drain last told, keeping the rest as playing on
    /// its own again needs it: the entries the drain played are skipped,
    /// its ramps are its own, and the next gate starts measuring the step
    /// length afresh.
    fn take_told(&mut self) {
        self.current_step = self.told_step.load(Ordering::Relaxed);
        self.skip_played();
        self.ramps.clear();
        self.first_gate_received = false;
        self.samples_since_gate = 0;
    }
}

impl BeatTable for ControlScheduler {
    fn id(&self) -> usize {
        self.table
    }

    fn clock_input(&self) -> &str {
        "clock"
    }

    fn plan(&self) -> u64 {
        self.plan
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn entry(&self, index: usize) -> Option<&TableEntry> {
        self.entries.get(index).map(|entry| &entry.entry)
    }

    fn set_played(&self, played: bool) {
        self.played.store(played, Ordering::Relaxed);
    }

    fn set_step(&self, step: i64) {
        self.told_step.store(step, Ordering::Relaxed);
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
mod refused_tests;
#[cfg(test)]
mod tests;
