//! Room for the ramps of the tables a live graph's drain plays, sized as a
//! change compiles (see `control_request::RampRoom`).
//!
//! The mirror keeps the room the drain holds once every publication so far
//! installs. A change whose schedules can run more ramps at once, summed
//! over the modules it leaves, carries more room in its publication; one
//! live schedule write that would need more publishes the topology
//! recompiled to carry it (see `publish::retarget`), and a change
//! publishing grows its room again for writes made since it compiled. Schedulers are the
//! only modules with tables a drain plays, and a scheduler's surface
//! knows the schedule it was last written.

use indexmap::IndexMap;
use std::collections::HashMap;

use super::{GraphChange, PreparedChange, SurfaceMap, TopologyMirror};
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
        let schedules = &self.rescheduling.schedules;
        table_ramps(&self.mirror, &self.surfaces, &self.ramps_written, schedules)
    }
}

impl PreparedChange {
    /// Under the publisher, as the change publishes: grows the room its
    /// publication carries for what its schedulers' schedules need now,
    /// when a write that fit the drain changed one since it compiled.
    /// Allocates.
    pub(crate) fn refit_ramp_room(&mut self) {
        let Some(publication) = self.publication.as_mut() else {
            return;
        };
        let (written, schedules) = (IndexMap::new(), &self.rescheduling.schedules);
        let needed = table_ramps(&self.mirror, &self.surfaces, &written, schedules);
        if RampRoom::fitting(needed) > RampRoom::fitting(self.mirror.table_ramps) {
            self.mirror.table_ramps = RampRoom::fitting(needed);
            publication.ramp_room = Some(RampRoom::new(needed));
        }
    }
}

/// The most ramps the tables of `mirror`'s modules can run at once, summed:
/// each scheduler's schedule as a live write leaves it (`written`, handed
/// over with the publication), else the more of what its surface last
/// recorded and what the change writes it (`schedules`), as either may
/// play while the publication installs.
fn table_ramps(
    mirror: &TopologyMirror,
    surfaces: &SurfaceMap,
    written: &IndexMap<String, usize>,
    schedules: &HashMap<String, String>,
) -> usize {
    let ramps = |id: &String| {
        written.get(id).copied().unwrap_or_else(|| {
            let surface = surfaces.get(id).and_then(|surface| surface.as_any());
            let controls = surface.and_then(|any| any.downcast_ref::<ControlSchedulerControls>());
            let recorded = controls.map_or(0, ControlSchedulerControls::table_ramps);
            let json = schedules.get(id);
            let rewritten = json.and_then(|json| ControlSchedulerControls::schedule_ramps(json));
            recorded.max(rewritten.unwrap_or(0))
        })
    };
    mirror.modules.keys().map(ramps).sum()
}
