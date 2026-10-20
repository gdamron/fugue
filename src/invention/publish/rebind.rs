//! Handing surviving schedulers the schedules a change re-resolves for them
//! (see [`Rescheduling`]): resolved under the publisher, where a schedule
//! write records itself, and packed into the publication, so the block that
//! installs it adopts them before any module runs, and no block writes a
//! scheduled value to an instance the change replaced.

use super::change::{PreparedChange, Rescheduling};
use crate::invention::graph::Handover;
use crate::invention::runtime::GraphCommandError;
use crate::modules::control_scheduler::{Rebind, Written};

/// Under the publisher, before `prepared` publishes: re-resolves every
/// surviving scheduler whose targets it touches and packs each schedule into
/// its publication. Returns the schedules the change writes them, to record
/// once it has published. Refuses the change when one no longer resolves;
/// nothing has changed then.
pub(super) fn pack(prepared: &mut PreparedChange) -> Result<Vec<Written>, GraphCommandError> {
    let rescheduling = std::mem::take(&mut prepared.rescheduling);
    let Some(publication) = prepared.publication.as_mut() else {
        return Ok(Vec::new());
    };
    let rebinds = Rescheduling::rebinds(&rescheduling, &prepared.surfaces, &prepared.built)?;
    let mut written = Vec::new();
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
        written.push(schedule);
    }
    Ok(written)
}
