//! Room for the ramps of the tables a drain plays, sized on a control
//! thread from those tables and handed to the drain whole.
//!
//! A table can run as many ramps at once as it has entries whose ramps
//! overlap ([`most_ramps`](crate::control_request::table::most_ramps)), and
//! a drain plays every table of its graph that it finds on a clock or on
//! edges, so its room is their sum: never less than [`RAMP_CAPACITY`],
//! never more than [`TABLE_RAMP_LIMIT`]. A control thread allocates the
//! room as an edit or a schedule write needs more than the drain holds,
//! and the edit's publication carries it; the audio thread takes it as the
//! publication installs, moving the ramps running into it, and the room it
//! leaves goes back with the publication to be freed off the audio thread.
//! Room only grows: a graph keeps the most its tables have needed.

use super::edges::EdgeRamps;
use super::Watches;
use crate::control_request::ramps::{Ramps, RAMP_CAPACITY, TABLE_RAMP_LIMIT};

/// Room for `tables` tables' ramps, played on clocks or on edges alike.
pub(crate) struct RampRoom {
    ramps: Ramps,
    edges: EdgeRamps,
}

impl RampRoom {
    /// Room for `tables` tables' ramps, kept between [`RAMP_CAPACITY`] and
    /// [`TABLE_RAMP_LIMIT`]. Allocates: control thread.
    pub(crate) fn new(tables: usize) -> Self {
        let tables = tables.clamp(RAMP_CAPACITY, TABLE_RAMP_LIMIT);
        Self {
            ramps: Ramps::with_tables(RAMP_CAPACITY, tables),
            edges: EdgeRamps::with_room(tables),
        }
    }

    /// The room `tables` tables' ramps need, as [`Self::new`] keeps it.
    pub(crate) fn fitting(tables: usize) -> usize {
        tables.clamp(RAMP_CAPACITY, TABLE_RAMP_LIMIT)
    }
}

impl Watches {
    /// The tables' ramps it runs at most, on clocks or on edges alike.
    pub(crate) fn table_room(&self) -> usize {
        self.ramps.table_room()
    }

    /// Takes `room` for the tables' ramps when it holds more, moving the
    /// ramps running into it: `room` is left holding the room this had, to
    /// free off the audio thread. Allocation-, free- and lock-free.
    pub(crate) fn take_room(&mut self, room: &mut RampRoom) {
        if room.ramps.table_room() <= self.ramps.table_room() {
            return;
        }
        self.ramps.take_room(&mut room.ramps);
        self.edges.take_room(&mut room.edges);
    }
}
