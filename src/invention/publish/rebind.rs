//! Handing surviving schedulers the schedules a change re-resolves for them
//! (see [`Rescheduling`]): resolved under the publisher, where a schedule
//! write records itself, and packed into the publication, so the block that
//! installs it adopts them before any module runs, and no block writes a
//! scheduled value to an instance the change replaced.

use std::collections::HashSet;

use super::change::{PreparedChange, Rescheduling};
use crate::invention::graph::Handover;
use crate::invention::runtime::GraphCommandError;
use crate::modules::control_scheduler::{Rebind, Written};

/// What [`pack`] handed surviving schedulers.
#[derive(Default)]
pub(crate) struct Packed {
    /// The schedules the change writes them, to record once it is
    /// adopted, and only then.
    pub(super) written: Vec<Written>,
    /// The schedulers handed one, by id.
    pub(super) rebound: Vec<String>,
    /// The modules the change rebuilds or removes.
    pub(super) touched: HashSet<String>,
}

impl Packed {
    /// Whether a payload write to `module_id`, resolved against the graph
    /// the change replaces to write `targets`, would race it: the change
    /// re-resolved the schedule the write replaces, or rebuilds or removes
    /// a module the write would go on writing.
    pub(super) fn races(&self, module_id: &str, targets: &[String]) -> bool {
        self.rebound.iter().any(|id| id == module_id)
            || targets.iter().any(|id| self.touched.contains(id))
    }
}

/// Under the publisher, before `prepared` publishes: re-resolves every
/// surviving scheduler whose targets it touches and packs each schedule into
/// its publication. Returns what it packed, the schedules the change writes
/// to record once it has published. Refuses the change when one no longer
/// resolves; nothing has changed then.
pub(super) fn pack(prepared: &mut PreparedChange) -> Result<Packed, GraphCommandError> {
    let rescheduling = std::mem::take(&mut prepared.rescheduling);
    let Some(publication) = prepared.publication.as_mut() else {
        return Ok(Packed::default());
    };
    let rebinds = Rescheduling::rebinds(&rescheduling, &prepared.surfaces, &prepared.built)?;
    let mut packed = Packed {
        written: Vec::with_capacity(rebinds.len()),
        rebound: Vec::with_capacity(rebinds.len()),
        touched: rescheduling.into_touched(),
    };
    publication.handovers.reserve_exact(rebinds.len());
    for Rebind {
        id,
        payload,
        written: schedule,
    } in rebinds
    {
        // A survivor, so the mirror has it, in the publication's order.
        let Some(module_idx) = prepared.mirror.modules.get_index_of(&id) else {
            continue;
        };
        publication
            .handovers
            .push(Handover::new(module_idx, Rebind::CONTROL, payload));
        packed.written.push(schedule);
        packed.rebound.push(id);
    }
    Ok(packed)
}
