//! Room for the ramps of the tables a live graph's drain plays, sized as a
//! change compiles (see `control_request::RampRoom`).
//!
//! The mirror keeps the room the drain holds once every publication so far
//! installs. A change whose schedules can run more ramps at once, summed
//! over the modules it leaves, carries more room in its publication; one
//! live schedule write that would need more publishes the topology
//! recompiled to carry it (see `publish::retarget`). Schedulers are the
//! only modules with tables a drain plays, and a scheduler's surface
//! knows the schedule it was last written.

use super::GraphChange;
use crate::control_request::RampRoom;
use crate::modules::ControlSchedulerControls;

impl GraphChange {
    /// Records that a live write leaves module `id`'s schedule running
    /// `ramps` ramps at once, ahead of its surface recording it. Returns
    /// whether the drain then needs more room for its tables' ramps, the
    /// change then editing the topology to carry it.
    pub(crate) fn write_ramps(&mut self, id: &str, ramps: usize) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.ramps_written.insert(id.to_string(), ramps);
        let grows = self.needs_room();
        if grows {
            self.edits += 1;
        }
        grows
    }

    /// The room for tables' ramps the change's publication carries, when
    /// the drain needs more than it holds; the mirror then holds it.
    /// Allocates.
    pub(super) fn grow_ramp_room(&mut self) -> Option<RampRoom> {
        if !self.needs_room() {
            return None;
        }
        let needed = self.table_ramps();
        self.mirror.table_ramps = RampRoom::fitting(needed);
        Some(RampRoom::new(needed))
    }

    fn needs_room(&self) -> bool {
        RampRoom::fitting(self.table_ramps()) > RampRoom::fitting(self.mirror.table_ramps)
    }

    /// The most ramps the tables of the modules the change leaves can run
    /// at once, summed.
    fn table_ramps(&self) -> usize {
        let ramps = |id: &String| {
            let written = self.ramps_written.get(id).copied();
            written.unwrap_or_else(|| {
                let surface = self.surfaces.get(id).and_then(|surface| surface.as_any());
                let controls =
                    surface.and_then(|any| any.downcast_ref::<ControlSchedulerControls>());
                controls.map_or(0, ControlSchedulerControls::table_ramps)
            })
        };
        self.mirror.modules.keys().map(ramps).sum()
    }
}
