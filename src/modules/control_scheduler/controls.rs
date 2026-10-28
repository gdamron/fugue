//! Thread-safe controls for the ControlScheduler module.
//!
//! `schedule` is a declared payload control: a write is parsed and resolved
//! here, on a control thread, and the resolved schedule handed to the
//! scheduler whole by its surface's route (a request on a live graph), so
//! the audio thread neither locks nor allocates to adopt it. What it was
//! last written as, and the attachment it resolves through, stay here
//! behind a lock only control threads take. `step` is read-only, mirrored
//! from the audio thread through an atomic.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use crate::audio_thread::ControlLock;
use crate::control_request::{
    most_ramps, Automation, ControlCells, ControlDecl, ControlIndex, ControlTable, DeclKind,
    RtValue,
};
use crate::invention::declared::{
    Checked, Declaration, DeclaredSurface, Queued, Route, WriteOptions, STALE,
};
use crate::payload::Payload;
use crate::{ControlMeta, ControlSurface, ControlValue, Module};

use super::resolved::{target_module_ids, ResolvedSchedule};
use super::schedule::{
    parse_schedule_json, resolve_schedule, ResolvedEntry, ScheduleEntry, SurfaceDirectory,
    SurfaceMap, WeakDirectory,
};

mod rebind;
pub(crate) use rebind::{Rebind, Written};

pub(super) const SCHEDULE: ControlIndex = ControlIndex(0);

const SCHEDULE_DESCRIPTION: &str = "Schedule as JSON: [{at_step, module, control, value, \
    ramp_steps?}] (at_step = clock rising-edge count, first edge = step 0)";

/// Only `schedule` is declared: a payload with no codec, since resolving it
/// needs the scheduler's attachment, so these controls build it themselves
/// (see [`ControlSurface::prepare_payload`]).
const DECLS: &[ControlDecl] = &[ControlDecl::new(
    "schedule",
    DeclKind::Payload,
    RtValue::Bool(false),
    SCHEDULE_DESCRIPTION,
)];

pub(super) static TABLE: ControlTable = ControlTable::of(DECLS);

/// Thread-safe controls for the ControlScheduler module.
///
/// Controls:
/// - `schedule` - The schedule as JSON (writable during playback)
/// - `step` - Read-only: current step index (-1 before the first clock edge)
#[derive(Clone)]
pub struct ControlSchedulerControls {
    declared: Arc<DeclaredSurface>,
    cells: Arc<ControlCells>,
    shared: Arc<Shared>,
}

struct Shared {
    /// Current step index maintained by the audio thread (-1 before the
    /// first gate edge).
    step: AtomicI64,
    /// The id of the schedule the scheduler adopted last (see
    /// `ResolvedSchedule::id`), stored by the audio thread.
    adopted: AtomicU64,
    /// Control threads only.
    state: ControlLock<ScheduleState>,
    /// Held across a whole `schedule` write, so concurrent writes reach
    /// the scheduler in the order they record their schedules. Taken
    /// before `state` and the publisher, never while holding a publisher or
    /// graph lock.
    writing: ControlLock<()>,
}

struct ScheduleState {
    /// The schedule as declared (config/JSON form). On a live graph, a
    /// write records it under the publisher, as it submits its payload, so
    /// a graph change sees the schedule the scheduler will be playing (see
    /// `ControlSchedulerControls::rebind`).
    spec: Vec<ScheduleEntry>,
    /// Module ids the schedule last resolved targets.
    targets: Vec<String>,
    /// The id of the schedule last handed on with `spec`: until the
    /// scheduler reports adopting it, it may still be playing an earlier
    /// one (a request in flight, or refused).
    recorded: u64,
    /// Runtime attachment: the scheduler's module id and the directory used
    /// to resolve schedule updates.
    attachment: Option<Attachment>,
    /// The module id of a throwaway copy that is never attached (an edit
    /// batch checks its writes on one), so it can still refuse a schedule
    /// that targets itself.
    unattached_id: Option<String>,
}

impl ScheduleState {
    /// Records `spec` as the schedule, and its targets.
    fn record(&mut self, spec: Vec<ScheduleEntry>) {
        self.targets = target_module_ids(&spec);
        self.spec = spec;
    }

    /// Records `spec` as the schedule, handed on as schedule `id`.
    fn record_handed(&mut self, spec: Vec<ScheduleEntry>, id: u64) {
        self.record(spec);
        self.recorded = id;
    }
}

/// How often a schedule write resolves again when a graph change keeps
/// publishing under it before giving up.
const STALE_RETRIES: usize = 8;

struct Attachment {
    own_id: String,
    directory: WeakDirectory,
}

impl ControlSchedulerControls {
    /// Creates controls holding an unresolved schedule. Resolution happens
    /// when the runtime attaches the scheduler (see [`Self::attach`]).
    pub(crate) fn new(spec: Vec<ScheduleEntry>) -> Self {
        let cells = Arc::new(ControlCells::new(DECLS.iter().map(|decl| decl.default)));
        Self {
            declared: Arc::new(DeclaredSurface::new(TABLE.clone(), cells.clone())),
            cells,
            shared: Arc::new(Shared {
                step: AtomicI64::new(-1),
                adopted: AtomicU64::new(0),
                state: ControlLock::new(ScheduleState {
                    spec,
                    targets: Vec::new(),
                    recorded: 0,
                    attachment: None,
                    unattached_id: None,
                }),
                writing: ControlLock::new(()),
            }),
        }
    }

    /// The cells the scheduler declares, shared with its surface.
    pub(super) fn cells(&self) -> Arc<ControlCells> {
        self.cells.clone()
    }

    /// Attaches the scheduler to its runtime: records its module id and the
    /// shared control-surface directory, then resolves the configured
    /// schedule. Returns an error (leaving the schedule unresolved) when any
    /// entry targets a missing module or control.
    pub(crate) fn attach(&self, own_id: &str, directory: &SurfaceDirectory) -> Result<(), String> {
        // A copy, so the directory is not held while a write is: a write
        // takes the directory after its own lock.
        let surfaces = directory.lock().unwrap().clone();
        self.attach_resolving(own_id, directory, &surfaces)
    }

    /// Attaches like [`Self::attach`], but resolves the schedule against
    /// `surfaces` (the directory as it will be once a pending graph change
    /// commits) rather than the directory's current contents. Later schedule
    /// edits re-resolve through `directory`.
    ///
    /// The scheduler is still building, so its resolved schedule waits in
    /// its surface until it is bound to run.
    pub(crate) fn attach_resolving(
        &self,
        own_id: &str,
        directory: &SurfaceDirectory,
        surfaces: &SurfaceMap,
    ) -> Result<(), String> {
        let _writing = self.shared.writing.lock().unwrap();
        let spec = self.shared.state.lock().unwrap().spec.clone();
        let resolved = self.resolve(&spec, own_id, surfaces)?;
        let id = self.deliver(resolved, &spec)?;
        let mut state = self.shared.state.lock().unwrap();
        state.attachment = Some(Attachment {
            own_id: own_id.to_string(),
            directory: Arc::downgrade(directory),
        });
        state.record_handed(spec, id);
        Ok(())
    }

    /// Names a throwaway copy that is never attached, for checks that need
    /// its own id (a schedule may not target its own scheduler).
    pub(crate) fn name_unattached(&self, own_id: &str) {
        self.shared.state.lock().unwrap().unattached_id = Some(own_id.to_string());
    }

    /// Replaces the schedule from JSON text, re-resolving against the
    /// attached directory and handing the result to the scheduler. On error
    /// the current schedule is left unchanged.
    ///
    /// A scheduler not yet attached (one an edit batch is building) keeps
    /// the parsed schedule, and resolves it when it is attached.
    ///
    /// On a live graph the schedule resolves against the directory as of a
    /// publisher generation, and a graph change publishing before the
    /// payload is submitted makes the write stale (it may have rebuilt a
    /// target): it resolves again.
    pub fn set_schedule_json(&self, json: &str) -> Result<(), String> {
        self.submit_schedule_json(json, &WriteOptions::default())
            .map(drop)
    }

    /// [`Self::set_schedule_json`] as `options` say, saying what the write
    /// saw as it was queued.
    fn submit_schedule_json(&self, json: &str, options: &WriteOptions) -> Result<Queued, String> {
        let spec = parse_schedule_json(json)?;
        let _writing = self.shared.writing.lock().unwrap();
        for _ in 0..STALE_RETRIES {
            let generation = self.declared.generation();
            let Some(resolved) = self.resolve_attached(&spec)? else {
                self.shared.state.lock().unwrap().record(spec);
                return Ok(Queued::default());
            };
            let schedule = ResolvedSchedule::new(resolved);
            let id = schedule.id;
            let payload = Payload::owned(Box::new(schedule));
            let record = || {
                let mut state = self.shared.state.lock().unwrap();
                state.record_handed(spec.clone(), id);
            };
            let checked = Checked {
                generation,
                targets: target_module_ids(&spec),
                ramps: spec_ramps(&spec),
                record: Box::new(record),
            };
            let write = ("schedule", payload, spec_json(&spec).into());
            match self.declared.write_payload_checked(write, checked, options) {
                Err(error) if error == STALE => continue,
                written => return written,
            }
        }
        Err(STALE.into())
    }

    /// Resolves `spec` through the attached directory, or `None` when the
    /// scheduler is not attached yet.
    fn resolve_attached(
        &self,
        spec: &[ScheduleEntry],
    ) -> Result<Option<Vec<super::schedule::ResolvedEntry>>, String> {
        let (own_id, directory) = {
            let state = self.shared.state.lock().unwrap();
            let Some(attachment) = state.attachment.as_ref() else {
                return Ok(None);
            };
            (attachment.own_id.clone(), attachment.directory.clone())
        };
        let directory = directory
            .upgrade()
            .ok_or("control_scheduler runtime is gone")?;
        let surfaces = directory.lock().unwrap();
        self.resolve(spec, &own_id, &surfaces).map(Some)
    }

    /// Resolves `spec` against `surfaces`.
    fn resolve(
        &self,
        spec: &[ScheduleEntry],
        own_id: &str,
        surfaces: &SurfaceMap,
    ) -> Result<Vec<ResolvedEntry>, String> {
        resolve_schedule(spec, own_id, surfaces)
    }

    /// Hands `resolved` to the scheduler by its surface's route, returning
    /// the id of the schedule handed on.
    fn deliver(
        &self,
        resolved: Vec<super::schedule::ResolvedEntry>,
        spec: &[ScheduleEntry],
    ) -> Result<u64, String> {
        let schedule = ResolvedSchedule::new(resolved);
        let id = schedule.id;
        let payload = Payload::owned(Box::new(schedule));
        self.declared
            .write_payload("schedule", payload, spec_json(spec).into())?;
        Ok(id)
    }

    /// Returns the schedule as JSON text.
    pub fn schedule_json(&self) -> String {
        spec_json(&self.shared.state.lock().unwrap().spec)
    }

    /// Module ids targeted by the resolved schedule, deduplicated. Used for
    /// process-order dependency discovery on topology changes.
    pub(crate) fn target_module_ids(&self) -> Vec<String> {
        self.shared.state.lock().unwrap().targets.clone()
    }

    /// The most ramps the schedule recorded can run at once: the room a
    /// drain playing it needs (see `control_request::RampRoom`).
    pub(crate) fn table_ramps(&self) -> usize {
        spec_ramps(&self.shared.state.lock().unwrap().spec)
    }

    /// [`Self::table_ramps`] of schedule `json`, or `None` when it does not
    /// parse (a write then refuses it).
    pub(crate) fn schedule_ramps(json: &str) -> Option<usize> {
        parse_schedule_json(json).ok().map(|spec| spec_ramps(&spec))
    }

    /// [`Self::target_module_ids`], or `None` while a control thread holds
    /// them: it never waits, so the audio thread may call it.
    pub(crate) fn try_target_module_ids(&self) -> Option<Vec<String>> {
        let state = self.shared.state.try_lock().ok()?;
        Some(state.targets.clone())
    }

    /// Holds the schedule state's lock, for a test proving the audio
    /// thread never waits for it.
    #[cfg(test)]
    pub(crate) fn hold_state_lock(&self) -> std::sync::MutexGuard<'_, impl Sized> {
        self.shared.state.lock().unwrap()
    }

    /// Current step index (-1 before the first gate edge).
    pub fn step(&self) -> i64 {
        self.shared.step.load(Ordering::Relaxed)
    }

    pub(crate) fn set_step(&self, step: i64) {
        self.shared.step.store(step, Ordering::Relaxed);
    }

    /// Reports that the scheduler adopted schedule `id`. Audio thread.
    pub(crate) fn set_adopted(&self, id: u64) {
        self.shared.adopted.store(id, Ordering::Release);
    }

    /// The id the scheduler checks its own schedule against.
    fn own_id(&self) -> Option<String> {
        let state = self.shared.state.lock().unwrap();
        state
            .attachment
            .as_ref()
            .map(|attachment| attachment.own_id.clone())
            .or_else(|| state.unattached_id.clone())
    }
}

/// Every lock these controls take, held (see
/// [`ControlSchedulerControls::held`]).
#[cfg(test)]
pub(crate) struct HeldLocks<'a> {
    _writing: std::sync::MutexGuard<'a, ()>,
    _state: std::sync::MutexGuard<'a, ScheduleState>,
}

#[cfg(test)]
impl ControlSchedulerControls {
    /// The surface its `schedule` writes go through.
    pub(crate) fn declared(&self) -> &DeclaredSurface {
        &self.declared
    }

    /// The id of the schedule last recorded as handed on.
    pub(crate) fn recorded(&self) -> u64 {
        self.shared.state.lock().unwrap().recorded
    }

    /// The id of the schedule the scheduler reports adopting last.
    pub(crate) fn adopted(&self) -> u64 {
        self.shared.adopted.load(Ordering::Acquire)
    }

    /// Takes every lock these controls take, for a test showing the
    /// scheduler never waits on one.
    pub(crate) fn held(&self) -> HeldLocks<'_> {
        HeldLocks {
            _writing: self.shared.writing.lock().unwrap(),
            _state: self.shared.state.lock().unwrap(),
        }
    }
}

/// The most ramps `spec` can run at once (see [`most_ramps`]).
fn spec_ramps(spec: &[ScheduleEntry]) -> usize {
    most_ramps(
        spec.iter()
            .map(|entry| (entry.at_step, entry.ramp_steps.unwrap_or(0))),
    )
}

fn spec_json(spec: &[ScheduleEntry]) -> String {
    serde_json::to_string(spec).unwrap_or_else(|_| "[]".to_string())
}

impl ControlSurface for ControlSchedulerControls {
    fn controls(&self) -> Vec<ControlMeta> {
        vec![
            ControlMeta::string("schedule", SCHEDULE_DESCRIPTION)
                .with_default(self.schedule_json()),
            ControlMeta::number(
                "step",
                "Read-only: current step index (-1 before the first clock edge)",
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

    fn control_targets(&self) -> Vec<String> {
        self.target_module_ids()
    }

    fn set_control(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "schedule" => self.set_schedule_json(value.as_string()?),
            "step" => Err("Control 'step' is read-only".to_string()),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    /// Parses a new schedule and resolves it against `surfaces`, the
    /// directory the write will land in, as the setter resolves it against
    /// the runtime's.
    fn validate_control(
        &self,
        key: &str,
        value: &ControlValue,
        surfaces: &SurfaceMap,
    ) -> Result<(), String> {
        match key {
            "schedule" => {
                let spec = parse_schedule_json(value.as_string()?)?;
                // A scheduler an edit batch is still adding is checked before
                // it is attached, so it does not know its own id yet. Its
                // targets are still checked against `surfaces`; that it does
                // not target itself is checked again once it is attached.
                let own_id = self.own_id();
                resolve_schedule(&spec, own_id.as_deref().unwrap_or(""), surfaces).map(drop)
            }
            "step" => crate::traits::read_only(key),
            _ => Err(format!("Unknown control: {}", key)),
        }
    }

    /// A development aliasing `schedule` hands the payload on itself: this
    /// builds it, resolved through the attached directory.
    #[allow(private_interfaces)]
    fn prepare_payload(
        &self,
        key: &str,
        value: &ControlValue,
    ) -> Option<Result<(Payload, ControlValue), String>> {
        if key != "schedule" {
            return None;
        }
        let prepared = || {
            let spec = parse_schedule_json(value.as_string()?)?;
            let resolved = self
                .resolve_attached(&spec)?
                .ok_or("control_scheduler is not attached")?;
            let payload = Payload::owned(Box::new(ResolvedSchedule::new(resolved)));
            Ok((payload, spec_json(&spec).into()))
        };
        Some(prepared())
    }

    /// Records a schedule a development handed on (see
    /// [`Self::prepare_payload`]), as reads report it. The payload is
    /// already delivered, so this only records: it never resolves again,
    /// and fails only on text that does not parse, which
    /// `prepare_payload` refused already.
    fn set_undeclared(&self, key: &str, value: ControlValue) -> Result<(), String> {
        match key {
            "schedule" => {
                let spec = parse_schedule_json(value.as_string()?)?;
                self.shared.state.lock().unwrap().record(spec);
                Ok(())
            }
            _ => self.set_control(key, value),
        }
    }

    #[allow(private_interfaces)]
    fn bind(&self, route: Route, module: &mut dyn Module) {
        self.declared.bind(route, module);
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
    fn submit(
        &self,
        key: &str,
        value: ControlValue,
        options: &WriteOptions,
    ) -> Result<Queued, String> {
        match key {
            "schedule" => self.submit_schedule_json(value.as_string()?, options),
            _ => self.set_control(key, value).map(|()| Queued::default()),
        }
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
