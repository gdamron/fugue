//! Ramps a table played on edges starts, measured in edges: over
//! `ramp_steps` N, the control is exactly `from + (to - from) * k / N` on
//! the sample of the k-th edge after its entry's, and between edges moves
//! on by the samples between the last two, written where each of its
//! target's segments starts, never past its next boundary. An entry on the
//! control, from any table, whoever plays it, ends a ramp there; a reset
//! rise or another plan ends the table's ramps where they stand.

use crate::control_request::ramps::{ramp_value, Ramps, RAMP_CAPACITY};
use crate::control_request::table::{BeatTable, Counted, TableEntry, TableValue};

/// A ramp an entry started, in edges.
struct EdgeRamp {
    table: usize,
    entry: usize,
    /// The module it writes, if installed.
    target: Option<usize>,
    from: f32,
    to: f32,
    total: u64,
    done: u64,
    /// When its entry began (see `TableTarget::begun`): an entry begun on
    /// its control since replaced it.
    begun: u64,
}

/// The ramps of every table played on edges. Single-threaded (the audio
/// thread), allocated once on a control thread and never grown.
pub(super) struct EdgeRamps(Vec<EdgeRamp>);

impl EdgeRamps {
    /// Room for [`RAMP_CAPACITY`] ramps. Allocates: control thread.
    pub(super) fn new() -> Self {
        Self(Vec::with_capacity(RAMP_CAPACITY))
    }

    /// Ends table `table`'s ramps on targets `mine` takes.
    pub(super) fn end(&mut self, table: usize, mine: &impl Fn(Option<usize>) -> bool) {
        self.0
            .retain(|ramp| ramp.table != table || !mine(ramp.target));
    }

    /// Whether table `table` has a ramp on a target `mine` takes.
    pub(super) fn any(&self, table: usize, mine: &impl Fn(Option<usize>) -> bool) -> bool {
        self.0
            .iter()
            .any(|ramp| ramp.table == table && mine(ramp.target))
    }

    /// Finds again the module each of table `id`'s ramps writes, from
    /// `target` of its entry's index.
    pub(super) fn retarget(&mut self, id: usize, target: impl Fn(usize) -> Option<usize>) {
        for ramp in self.0.iter_mut().filter(|ramp| ramp.table == id) {
            ramp.target = target(ramp.entry);
        }
    }

    /// At an edge of table `id`, brings each of its ramps on targets
    /// `mine` takes to its next boundary, exactly; one arriving, or held
    /// off by a higher tier, ends.
    pub(super) fn edge(
        &mut self,
        id: usize,
        table: &dyn BeatTable,
        mine: &impl Fn(Option<usize>) -> bool,
    ) {
        self.0.retain_mut(|ramp| {
            if ramp.table != id || !mine(ramp.target) {
                return true;
            }
            let Some(entry) = table.entry(ramp.entry) else {
                return false;
            };
            if entry.target.begun() > ramp.begun {
                return false;
            }
            ramp.done += 1;
            if ramp.done >= ramp.total {
                entry.target.write(TableValue::Number(ramp.to));
                return false;
            }
            let progress = ramp.done as f64 / ramp.total as f64;
            entry
                .target
                .write(TableValue::Number(ramp_value(ramp.from, ramp.to, progress)))
        });
    }

    /// Ends table `id`'s ramp on the control `entry` writes, as a newer
    /// change supersedes it (a legacy control's too, which notes no begin).
    pub(super) fn supersede(&mut self, id: usize, table: &dyn BeatTable, entry: &TableEntry) {
        self.0.retain(|ramp| {
            ramp.table != id
                || !table
                    .entry(ramp.entry)
                    .is_some_and(|other| other.target.same(&entry.target))
        });
    }

    /// Starts entry `index` of table `id`'s ramp (on module `target`), from
    /// where its control stands. Returns false for an entry with none: it
    /// jumps to its value. A control holding no number is skipped, as a
    /// scheduler always skipped it; with no room the entry jumps to its
    /// value, counted in `beat_ramps`.
    pub(super) fn start(
        &mut self,
        id: usize,
        index: usize,
        target: Option<usize>,
        entry: &TableEntry,
        beat_ramps: &Ramps,
    ) -> bool {
        let TableValue::Number(to) = entry.value else {
            return false;
        };
        if entry.ramp_steps == 0 {
            return false;
        }
        let Some(from) = entry.target.current() else {
            return true;
        };
        if self.0.len() >= RAMP_CAPACITY {
            beat_ramps.crowded().record();
            entry.target.write(entry.value);
            return true;
        }
        self.0.push(EdgeRamp {
            table: id,
            entry: index,
            target,
            from,
            to,
            total: entry.ramp_steps,
            done: 0,
            begun: entry.target.begun(),
        });
        true
    }

    /// Writes the value of each of table `id`'s ramps on targets `mine`
    /// takes, `since` samples after the edge `counted` last counted: moved
    /// on from its last boundary by the samples between the last two edges,
    /// short of the next. One held off by a higher tier ends.
    pub(super) fn between(
        &mut self,
        id: usize,
        table: &dyn BeatTable,
        counted: &Counted,
        since: u32,
        mine: &impl Fn(Option<usize>) -> bool,
    ) {
        let duration = counted.interval.max(1) as f32;
        let frac = (since as f32 / duration).min(1.0);
        self.0.retain(|ramp| {
            if ramp.table != id || !mine(ramp.target) {
                return true;
            }
            let Some(entry) = table.entry(ramp.entry) else {
                return false;
            };
            if entry.target.begun() > ramp.begun {
                return false;
            }
            let progress = ((ramp.done as f32 + frac) / ramp.total as f32).min(1.0);
            entry.target.write(TableValue::Number(ramp_value(
                ramp.from,
                ramp.to,
                progress.into(),
            )))
        });
    }
}
