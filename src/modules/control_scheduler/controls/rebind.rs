//! Re-resolving a surviving scheduler's schedule for a graph change that
//! rebuilds or removes a module it targets.
//!
//! Resolved entries hold their targets' surfaces and automation cells, so a
//! target the change rebuilds would go on being written through the
//! instance it replaced. The change re-resolves the schedule against the
//! surfaces it will leave ([`ControlSchedulerControls::rebind`]), which
//! refuses a change that removes a target the schedule still names, and the
//! commit packs the result into the publication, so the block that installs
//! the new topology adopts it too (see `invention::publish::rebind`).

use std::collections::HashSet;

use super::{ControlSchedulerControls, ResolvedSchedule, SurfaceMap, SCHEDULE};
use crate::control_request::ControlIndex;
use crate::modules::control_scheduler::schedule::{
    parse_schedule_json, resolve_schedule, ScheduleEntry,
};
use crate::payload::Payload;

/// A surviving scheduler's schedule, re-resolved for a change.
pub(crate) struct Rebind {
    /// The scheduler's module id.
    pub(crate) id: String,
    pub(crate) payload: Payload,
    pub(crate) written: Written,
}

impl Rebind {
    /// The control the payload is for.
    pub(crate) const CONTROL: ControlIndex = SCHEDULE;
}

/// A schedule the change writes to a rebound scheduler, to record once the
/// change commits.
pub(crate) struct Written(Option<(ControlSchedulerControls, Vec<ScheduleEntry>)>);

impl Written {
    /// Records the schedule as the scheduler's, once the change has
    /// published. Under the publisher, as a write records.
    pub(crate) fn record(self) {
        if let Some((controls, spec)) = self.0 {
            controls.shared.state.lock().unwrap().record(spec);
        }
    }
}

impl ControlSchedulerControls {
    /// Re-resolves the schedule against `surfaces`, the directory a change
    /// will leave, when the change `touched` (rebuilds or removes) a module
    /// it targets: `json` when the change also writes one, otherwise the
    /// schedule it plays. `None` for a scheduler whose targets the change
    /// leaves alone (a schedule it writes then lands after it, as any write
    /// does), or one not attached yet. Fails when an entry's target or
    /// control is gone. Changes nothing.
    ///
    /// The commit calls it under the publisher, where a write records its
    /// schedule, so it re-resolves the schedule the scheduler will be
    /// playing when the change installs.
    pub(crate) fn rebind(
        &self,
        json: Option<&str>,
        touched: &HashSet<String>,
        surfaces: &SurfaceMap,
    ) -> Result<Option<Rebind>, String> {
        let (own_id, current) = {
            let state = self.shared.state.lock().unwrap();
            let Some(attachment) = state.attachment.as_ref() else {
                return Ok(None);
            };
            if !state.targets.iter().any(|id| touched.contains(id)) {
                return Ok(None);
            }
            let current = json.is_none().then(|| state.spec.clone());
            (attachment.own_id.clone(), current)
        };
        let (schedule, written) = match (json, current) {
            (Some(json), _) => {
                let spec = parse_schedule_json(json)?;
                let resolved = resolve_schedule(&spec, &own_id, surfaces)?;
                (
                    ResolvedSchedule::new(resolved),
                    Written(Some((self.clone(), spec))),
                )
            }
            (None, spec) => {
                let resolved = resolve_schedule(&spec.unwrap_or_default(), &own_id, surfaces)?;
                (ResolvedSchedule::new(resolved), Written(None))
            }
        };
        Ok(Some(Rebind {
            id: own_id,
            payload: Payload::owned(Box::new(schedule)),
            written,
        }))
    }
}
