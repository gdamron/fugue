//! Re-resolving surviving schedulers whose targets a change rebuilds or
//! removes (see `ControlSchedulerControls::rebind`).

use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};

use super::{BuiltModule, GraphChange, SurfaceMap};
use crate::invention::runtime::GraphCommandError;
use crate::modules::control_scheduler::Rebind;
use crate::modules::ControlSchedulerControls;

/// What a change does that surviving schedulers re-resolve for.
#[derive(Default)]
pub(crate) struct Rescheduling {
    /// Ids an upsert or a remove touched.
    pub(super) touched: HashSet<String>,
    /// Schedules the change also writes to surviving schedulers, by id.
    pub(super) schedules: HashMap<String, String>,
}

impl Rescheduling {
    /// Re-resolves, against `surfaces` (the directory as the change will
    /// leave it), the schedule of every surviving scheduler that targets a
    /// module the change touched. Refuses the change, with nothing changed,
    /// when one no longer resolves: removing a module a schedule still
    /// targets, without writing a schedule that no longer does, is an
    /// error.
    pub(crate) fn rebinds(
        &self,
        surfaces: &SurfaceMap,
        built: &IndexMap<String, BuiltModule>,
    ) -> Result<Vec<Rebind>, GraphCommandError> {
        let mut rebinds = Vec::new();
        if self.touched.is_empty() {
            return Ok(rebinds);
        }
        for (id, surface) in surfaces {
            if built.contains_key(id) {
                continue;
            }
            let Some(controls) = surface
                .as_any()
                .and_then(|any| any.downcast_ref::<ControlSchedulerControls>())
            else {
                continue;
            };
            let json = self.schedules.get(id).map(String::as_str);
            let rebind = controls
                .rebind(json, &self.touched, surfaces)
                .map_err(|error| {
                    GraphCommandError::ModuleBuildFailed(format!(
                        "module '{id}': {error}; the change rebuilds or removes a module its \
                         schedule targets, so write the schedule in the same change"
                    ))
                })?;
            rebinds.extend(rebind);
        }
        Ok(rebinds)
    }
}

impl GraphChange {
    /// Records that the change also writes `json` to surviving scheduler
    /// `id`'s `schedule`, so a scheduler whose targets it touches
    /// re-resolves that one rather than the schedule it replaces.
    pub(crate) fn reschedule(&mut self, id: &str, json: &str) {
        self.rescheduling
            .schedules
            .insert(id.to_string(), json.to_string());
    }
}
