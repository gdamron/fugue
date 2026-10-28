//! Tables of automation timed in steps of a clock: a scheduler's compiled
//! schedule, which the graph's request drain plays in place, at the sample
//! each step's beat falls on (see `super::watch::cursor`).
//!
//! An entry writes as automation does, through its target's automation
//! slot, at automation's tier, so a higher tier's write holds it off until
//! the table's next entry on it (see `super::hold`). Only a declared
//! control can be a target: one its module does not declare is refused
//! when the schedule resolves.

use super::automation::Automation;
use super::cells::ControlCells;
use super::request::ControlIndex;

/// What an entry writes: a number or a boolean, so writing one allocates
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TableValue {
    Number(f32),
    Bool(bool),
}

/// Where an entry writes, resolved on a control thread: a declared
/// control, through its automation slot.
#[derive(Clone)]
pub(crate) struct TableTarget(pub(crate) Automation);

impl TableTarget {
    /// The table's next event on the control, on frame `frame` of the
    /// current frame scope: frees it from a higher tier's hold, unless
    /// that write was due at its sample or after it (see
    /// `Automation::begin`).
    pub(crate) fn begin(&self, frame: usize) {
        self.0.begin(frame);
    }

    /// When an entry from any table last began on the control (see
    /// `Automation::begun`): a ramp noting it ends once it grows.
    #[inline]
    pub(crate) fn begun(&self) -> u64 {
        self.0.begun()
    }

    /// Writes `value`, returning false when a higher tier holds the
    /// control and it was dropped. Allocation-, free- and lock-free.
    pub(crate) fn write(&self, value: TableValue) -> bool {
        match value {
            TableValue::Number(number) => self.0.write_number(number),
            TableValue::Bool(flag) => self.0.write_bool(flag),
        }
    }

    /// The control's number as automation last left it: where a ramp
    /// starts. `None` for one holding no number.
    pub(crate) fn current(&self) -> Option<f32> {
        self.0.current()
    }

    /// Whether `self` and `other` reach a control in common: the same
    /// control, or, through a development, an inner control both reach.
    pub(crate) fn same(&self, other: &Self) -> bool {
        let mut shared = false;
        leaves(&self.0, &mut |cells, index| {
            leaves(&other.0, &mut |other, other_index| {
                shared |= std::ptr::eq(cells, other) && index == other_index;
            });
        });
        shared
    }
}

/// Calls `f` with the cells and index of every control `target` writes:
/// its own, or a development's aliases, to the leaf.
fn leaves(target: &Automation, f: &mut dyn FnMut(&ControlCells, ControlIndex)) {
    match &target.aliases {
        Some(aliases) => aliases.iter().for_each(|alias| leaves(alias, f)),
        None => f(&target.cells, target.index),
    }
}

/// One entry of a table: `value` written at step `step`, or glided to from
/// where the control stands over `ramp_steps` steps.
#[derive(Clone)]
pub(crate) struct TableEntry {
    pub(crate) step: u64,
    pub(crate) ramp_steps: u64,
    pub(crate) value: TableValue,
    pub(crate) target: TableTarget,
}

/// The most ramps a table of entries at `spans` (each entry's step and
/// ramp steps) can run at once: the most whose ramps overlap, a ramp
/// running from its step through its end step, both included. A count to
/// size a drain's room by, on a control thread: it ignores that a later
/// entry on the same control ends a ramp, so it never counts short.
/// Allocates.
pub(crate) fn most_ramps(spans: impl IntoIterator<Item = (u64, u64)>) -> usize {
    // Each ramp's start (-1, so at one step it sorts before an end) and
    // end (+1).
    let mut edges: Vec<(u64, i8)> = Vec::new();
    for (step, ramp_steps) in spans.into_iter().filter(|&(_, ramp)| ramp > 0) {
        edges.push((step, -1));
        edges.push((step.saturating_add(ramp_steps), 1));
    }
    edges.sort_unstable();
    let (mut running, mut most) = (0usize, 0usize);
    for (_, edge) in edges {
        if edge < 0 {
            running += 1;
            most = most.max(running);
        } else {
            running -= 1;
        }
    }
    most
}

/// [`most_ramps`] of `table`'s entries. Allocates: control thread.
pub(crate) fn table_ramps(table: &dyn BeatTable) -> usize {
    let entries = (0..table.len()).filter_map(|index| table.entry(index));
    most_ramps(entries.map(|entry| (entry.step, entry.ramp_steps)))
}

/// A module's count of its clock input's rising edges (see
/// [`BeatTable::counted`]), as the drain carries it on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Counted {
    /// The step it is in: -1 before its first edge, and after a reset.
    pub(crate) step: i64,
    /// Its clock and reset inputs' last levels: an edge rises past 0.5.
    pub(crate) gate: f32,
    pub(crate) reset: f32,
    /// Samples since its last edge, the samples between the two edges
    /// before that (the step length a ramp moves on by between edges), and
    /// whether an edge came since it started or reset (only the next one
    /// measures a step).
    pub(crate) since: u32,
    pub(crate) interval: u32,
    pub(crate) measured: bool,
}

impl Default for Counted {
    fn default() -> Self {
        Self {
            step: -1,
            gate: 0.0,
            reset: 0.0,
            since: 0,
            interval: 1,
            measured: false,
        }
    }
}

/// Who plays a table's entries (see [`BeatTable::set_played`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Player {
    /// Its module, counting its gates and writing as it processes.
    Module = 0,
    /// The drain, on the clock gate its clock input carries.
    Clock = 1,
    /// The drain, on the steps its module counts on its clock input's
    /// rising edges and reports on its step output (see
    /// `super::watch::edges`).
    Edges = 2,
}

/// A table a module plays automation from, read in place by the drain on
/// the audio thread: between `process` calls, allocation-, free- and
/// lock-free.
///
/// On a clock, its step `n` falls on its clock's `n`-th gate counted from
/// where the drain first saw it, and again from beat 0 after every reset
/// of the clock. On any other signal its module counts the rising edges,
/// and the drain plays each entry on the sample of its step's edge. A
/// module whose table no drain plays (one inside a development) plays it
/// itself, on its own gate.
pub(crate) trait BeatTable {
    /// Tells this table from every other for its module's life.
    fn id(&self) -> usize;

    /// The input its clock's gate comes in on: the clock output connected
    /// there sets the length of a step.
    fn clock_input(&self) -> &str;

    /// Changes when another plan replaces the entries; a retarget of the
    /// same plan (new instances behind the same targets) keeps it.
    fn plan(&self) -> u64;

    /// How many entries it has.
    fn len(&self) -> usize;

    /// Entry `index`, in step order (ties in the order written).
    fn entry(&self, index: usize) -> Option<&TableEntry>;

    /// Told who plays it: until a drain does, its module does.
    fn set_played(&self, player: Player);

    /// The input a rise on restarts its count, for one its module counts
    /// (see [`Self::counted`]).
    fn reset_input(&self) -> Option<&str> {
        None
    }

    /// The level its module's input `port` holds while nothing is
    /// connected to it: 0, or a value written to it, constant through a
    /// segment.
    fn held_input(&self, port: usize) -> f32 {
        let _ = port;
        0.0
    }

    /// How far its module counted its clock input's rising edges by the
    /// end of its last block: where a drain starting to play it on its
    /// edges takes up the count.
    fn counted(&self) -> Counted {
        Counted::default()
    }

    /// The id of the module entry `index` writes, when it names one: a
    /// drain playing it on its edges writes each entry just before that
    /// module computes the entry's sample.
    fn target_module(&self, index: usize) -> Option<&str> {
        let _ = index;
        None
    }

    /// Told, by the drain playing it, the step its clock's next sample is
    /// in (-1 before step 0), at every pass and on each step's sample: a
    /// module's `step` output follows its clock's position. `began`: that
    /// sample begins the step (its beat, whose gate rises there).
    fn set_step(&self, step: i64, began: bool);
}

#[cfg(test)]
mod tests;
