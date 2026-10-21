//! Tables of automation timed in steps of a clock: a scheduler's compiled
//! schedule, which the graph's request drain plays in place, at the sample
//! each step's beat falls on (see `super::watch::cursor`).
//!
//! An entry writes as automation does, through its own target: a declared
//! control through its automation slot, at automation's tier, so a higher
//! tier's write holds it off until the table's next entry on it (see
//! `super::hold`); a legacy control (one its module does not declare)
//! through its surface's setter, as a scheduler always wrote one, with no
//! tiers and no holds. A legacy write is allocation-, free- and lock-free
//! too ([`crate::ControlSurface::set_scheduled`]): one finding a control
//! thread holding the lock it needs waits to be tried again, in order (see
//! [`super::legacy::LegacyWrites`]), and one its surface cannot make is
//! refused and counted, never waited on or formatted.

use std::sync::Arc;

use super::automation::Automation;
use super::cells::ControlCells;
use super::event::EventCounter;
use super::legacy::{LegacyControls, LegacyWrites};
use super::request::ControlIndex;
use crate::ControlSurface;

/// What an entry writes: a number or a boolean, so writing one allocates
/// nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TableValue {
    Number(f32),
    Bool(bool),
}

/// Where an entry writes, resolved on a control thread.
#[derive(Clone)]
pub(crate) enum TableTarget {
    /// A declared control, through its automation slot.
    Declared(Automation),
    /// A legacy control, through its surface's scheduled setter.
    Legacy {
        surface: Arc<dyn ControlSurface + Send + Sync>,
        control: String,
        /// Its schedule's legacy writes, keeping those that wait, and its
        /// index there.
        writes: Arc<LegacyWrites>,
        index: usize,
    },
}

impl TableTarget {
    /// The table's next event on the control, on frame `frame` of the
    /// current frame scope: frees a declared one from a higher tier's
    /// hold, unless that write was due at its sample or after it (see
    /// `Automation::begin`). A legacy control has no holds.
    pub(crate) fn begin(&self, frame: usize) {
        if let Self::Declared(target) = self {
            target.begin(frame);
        }
    }

    /// A legacy control written through `surface`'s scheduled setter, on
    /// its own: a schedule's share their writes (see
    /// [`LegacyControls`]). Control thread.
    #[cfg(test)]
    pub(crate) fn legacy(surface: Arc<dyn ControlSurface + Send + Sync>, control: String) -> Self {
        let mut controls = LegacyControls::default();
        let index = controls.index(&surface, &control);
        controls.build().target(index, &surface, &control)
    }

    /// Writes `value`, returning false when a higher tier holds the
    /// control and it was dropped. Allocation-, free- and lock-free: a
    /// legacy write finding its surface busy waits to be tried again, one
    /// its surface refuses is counted (see [`LegacyWrites`]), and both
    /// return true, as a legacy control has no holds to end a ramp on.
    pub(crate) fn write(&self, value: TableValue) -> bool {
        match (self, value) {
            (Self::Declared(target), TableValue::Number(number)) => target.write_number(number),
            (Self::Declared(target), TableValue::Bool(flag)) => target.write_bool(flag),
            (Self::Legacy { writes, index, .. }, value) => {
                writes.write(*index, value);
                true
            }
        }
    }

    /// The control's number as automation last left it: where a ramp
    /// starts. `None` for one holding no number, or a legacy one its
    /// surface could not read without waiting (counted as refused).
    #[cfg(test)]
    pub(crate) fn current(&self) -> Option<f32> {
        self.current_after(&[])
    }

    /// [`Self::current`], for a legacy control as the writes still waiting
    /// in `before` (the schedules its own replaced, oldest first) will
    /// leave it, unless its own schedule has one waiting there too.
    pub(crate) fn current_after(&self, before: &[Arc<LegacyWrites>]) -> Option<f32> {
        match self {
            Self::Declared(target) => target.current(),
            Self::Legacy { writes, index, .. } => writes.read(*index, before),
        }
    }

    /// A legacy target's schedule's writes; `None` for a declared one.
    pub(crate) fn legacy_writes(&self) -> Option<&Arc<LegacyWrites>> {
        match self {
            Self::Declared(_) => None,
            Self::Legacy { writes, .. } => Some(writes),
        }
    }

    /// Counts a legacy target's writes that never landed and ramp starts
    /// refused (see [`LegacyWrites::refused`]); `None` for a declared one.
    /// Read it through an `EventCursor`.
    pub(crate) fn refused(&self) -> Option<&EventCounter> {
        self.legacy_writes().map(|writes| writes.refused())
    }

    /// Whether `self` and `other` reach a control in common: the same
    /// control, or, through a development, an inner declared control both
    /// reach. Legacy controls compare by surface and name, as a scheduler
    /// always compared them: two names a development forwards to one inner
    /// legacy control stay apart.
    pub(crate) fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Declared(a), Self::Declared(b)) => {
                let mut shared = false;
                leaves(a, &mut |cells, index| {
                    leaves(b, &mut |other, other_index| {
                        shared |= std::ptr::eq(cells, other) && index == other_index;
                    });
                });
                shared
            }
            (
                Self::Legacy {
                    surface, control, ..
                },
                Self::Legacy {
                    surface: other,
                    control: other_control,
                    ..
                },
            ) => {
                let (a, b) = (Arc::as_ptr(surface), Arc::as_ptr(other));
                std::ptr::addr_eq(a, b) && control == other_control
            }
            _ => false,
        }
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

/// A table a module plays automation from, read in place by the drain on
/// the audio thread: between `process` calls, allocation-, free- and
/// lock-free.
///
/// Its step `n` falls on its clock's `n`-th gate counted from where the
/// drain first saw it, and again from beat 0 after every reset of the
/// clock. A module whose table no drain plays (one inside a development)
/// plays it itself, on its own gate.
pub(crate) trait BeatTable {
    /// Tells this table from every other for its module's life.
    fn id(&self) -> usize;

    /// Tries again the writes to legacy controls a busy surface refused
    /// (see [`LegacyWrites`]): at the start of every pass, before its
    /// entries due then write.
    fn retry_legacy(&self) {}

    /// Counts one of its entries' ramps finding no room, as the drain's
    /// `Ramps::crowded` does, for its module to show.
    fn crowded(&self) {}

    /// The schedules it replaced whose writes still wait to land, oldest
    /// first: a ramp starts where they will leave its control.
    fn draining(&self) -> &[Arc<LegacyWrites>] {
        &[]
    }

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

    /// Told whether a drain plays it: until one does, its module does.
    fn set_played(&self, played: bool);

    /// Told, by the drain playing it, the step its clock's next sample is
    /// in (-1 before step 0), at every pass and on each step's sample: a
    /// module's `step` output follows its clock's position.
    fn set_step(&self, step: i64);
}

#[cfg(test)]
mod tests;
