//! A schedule's writes to legacy controls, and those waiting to land.
//!
//! A legacy surface refuses a scheduled write as [`LegacyRefusal::Busy`]
//! when a control thread holds the lock it needs, or [`LegacyRefusal::Full`]
//! when its note queue has no room, rather than wait (see
//! [`crate::ControlSurface::set_scheduled`]). Dropping it would lose a note,
//! or leave one sounding, so the schedule keeps it, in room allocated on a
//! control thread, and tries it again at the start of every segment the
//! scheduler or the drain plays, until it lands: a block or so late.
//!
//! - **Order.** A surface's writes land in the order they were made, from
//!   whichever schedules made them: while one waits, every later write to
//!   that surface waits behind it, so a `note_off` never lands before its
//!   `note_on`, even one another scheduler made (see [`gate`]).
//! - **Coalescing.** A value's waiting write gives way to a newer one to the
//!   same control when no other write to its surface came between them, so
//!   a ramp waiting takes one slot, and a write another depends on (a
//!   melody's `degree_count` before a `degree.N`) is not lost. An event
//!   ([`crate::ControlSurface::scheduled_event`]: a note, a play) is never
//!   merged: each waits its turn.
//! - **Bound.** At most [`RETRY_CAPACITY`] writes wait, the last
//!   [`EVENT_ROOM`] of them events only. A value finding no room gives way
//!   to its control's newer write after all, if no event to its surface came
//!   between them; else it is dropped. A value is tried again at least once,
//!   and then for at most the scheduler's limit (10 ms of samples). An event
//!   waits as long as it takes, and is dropped only for want of room. A
//!   write dropped is counted in [`LegacyWrites::refused`], as is one its
//!   surface refuses outright (a control it has not got, a value it does
//!   not take).
//! - **Fan-out.** A development's control reaching several aliases (see
//!   [`crate::ControlSurface::scheduled_aliases`]) is written alias by
//!   alias, so one alias busy waits alone: the others take the write once.
//! - **Adoption.** A schedule replaced keeps trying its waiting writes
//!   until they land, ahead of its successor's (the scheduler keeps it, see
//!   `ControlScheduler::adopt`), and a ramp the successor starts on one of
//!   their controls starts where they will leave it. A scheduler removed
//!   drops what it has waiting (see `Module::leave`).
//!
//! Its state is atomics, so it is `Sync`, but one thread at a time writes
//! through it: the audio thread, or an offline render's.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use super::LegacyRefusal;
use crate::control_request::table::{TableTarget, TableValue};
use crate::control_request::EventCounter;
use crate::ControlSurface;

mod gate;
use gate::{Gate, IDLE};

/// The most writes one schedule keeps waiting.
pub(crate) const RETRY_CAPACITY: usize = 32;

/// The waiting room only events may take: a ramp waiting never crowds out
/// the `note_off` after it.
const EVENT_ROOM: usize = 8;

type Surface = Arc<dyn ControlSurface + Send + Sync>;

/// One legacy control a schedule writes, and the surface it is on.
struct Control {
    surface: Surface,
    key: String,
    /// The same for every control on one surface: its writes keep order.
    group: usize,
    /// Whether a write is an event, never merged nor given up on.
    event: bool,
}

/// A surface a schedule writes: the order its writes there keep with
/// every other schedule's, and the schedule's lane in it.
struct Group {
    gate: Arc<Gate>,
    lane: usize,
}

/// A write's wait before its first retry: not yet aged.
const FRESH: u32 = u32::MAX;

/// A write waiting: its control, value, the samples it has waited and its
/// ticket at its surface's gate.
#[derive(Clone, Copy)]
struct Kept {
    index: usize,
    value: TableValue,
    waited: u32,
    ticket: u64,
}

/// Room for one [`Kept`] write.
#[derive(Default)]
struct Slot {
    control: AtomicU32,
    /// The value's bits, and whether it is a boolean (bit 32).
    value: AtomicU64,
    waited: AtomicU32,
    ticket: AtomicU64,
}

impl Slot {
    fn load(&self) -> Kept {
        let bits = self.value.load(Ordering::Relaxed);
        let value = match bits >> 32 {
            0 => TableValue::Number(f32::from_bits(bits as u32)),
            _ => TableValue::Bool(bits & 1 == 1),
        };
        Kept {
            index: self.index(),
            value,
            waited: self.waited.load(Ordering::Relaxed),
            ticket: self.ticket.load(Ordering::Relaxed),
        }
    }

    fn index(&self) -> usize {
        self.control.load(Ordering::Relaxed) as usize
    }

    fn store(&self, kept: Kept) {
        self.control.store(kept.index as u32, Ordering::Relaxed);
        self.set_value(kept.value);
        self.waited.store(kept.waited, Ordering::Relaxed);
        self.ticket.store(kept.ticket, Ordering::Relaxed);
    }

    fn set_value(&self, value: TableValue) {
        let bits = match value {
            TableValue::Number(number) => number.to_bits() as u64,
            TableValue::Bool(flag) => 1 << 32 | flag as u64,
        };
        self.value.store(bits, Ordering::Relaxed);
    }
}

/// Whether `a` and `b` are one surface.
fn same(a: &Surface, b: &Surface) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

/// The legacy controls one schedule writes, collected on a control thread
/// as it resolves: each entry's target, and the controls it reaches.
#[derive(Default)]
pub(crate) struct LegacyControls {
    /// Every control reached, once.
    leaves: Vec<(Surface, String)>,
    /// The controls each target reaches: `fanout[start..end]`.
    targets: Vec<(usize, usize)>,
    fanout: Vec<usize>,
}

impl LegacyControls {
    /// The index of a target writing `key` on `surface`, through to the
    /// controls a development's aliases reach.
    pub(crate) fn index(&mut self, surface: &Surface, key: &str) -> usize {
        let start = self.fanout.len();
        self.reach(surface, key);
        self.targets.push((start, self.fanout.len()));
        self.targets.len() - 1
    }

    fn reach(&mut self, surface: &Surface, key: &str) {
        if let Some(aliases) = surface.scheduled_aliases(key) {
            for (inner, inner_key) in aliases {
                self.reach(&inner, &inner_key);
            }
            return;
        }
        let at = (self.leaves.iter())
            .position(|(other, other_key)| same(surface, other) && other_key == key);
        let leaf = at.unwrap_or_else(|| {
            self.leaves.push((surface.clone(), key.to_string()));
            self.leaves.len() - 1
        });
        self.fanout.push(leaf);
    }

    /// Whether no target was indexed: the schedule writes no legacy
    /// control, and needs no [`LegacyWrites`].
    pub(crate) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// The schedule's writes, with room for those that wait, and a lane at
    /// the gate of every surface they reach. Allocates and locks.
    pub(crate) fn build(self) -> Arc<LegacyWrites> {
        let mut surfaces: Vec<&Surface> = Vec::new();
        let mut groups = Vec::new();
        let mut controls = Vec::with_capacity(self.leaves.len());
        for (surface, key) in &self.leaves {
            let group = surfaces.iter().position(|seen| same(seen, surface));
            let group = group.unwrap_or_else(|| {
                let gate = Gate::of(surface);
                let lane = gate.claim();
                groups.push(Group { gate, lane });
                surfaces.push(surface);
                surfaces.len() - 1
            });
            controls.push(Control {
                surface: surface.clone(),
                key: key.clone(),
                group,
                event: surface.scheduled_event(key),
            });
        }
        Arc::new(LegacyWrites {
            controls: controls.into(),
            groups: groups.into(),
            targets: self.targets.into(),
            fanout: self.fanout.into(),
            slots: (0..RETRY_CAPACITY).map(|_| Slot::default()).collect(),
            waiting: AtomicUsize::new(0),
            refused: EventCounter::new(),
            taken: AtomicU32::new(0),
        })
    }
}

/// A schedule's writes to its legacy controls, and those waiting to land
/// (see the module docs). Allocation-, free- and lock-free once built.
pub(crate) struct LegacyWrites {
    /// The controls the schedule reaches, and what each target reaches.
    controls: Box<[Control]>,
    targets: Box<[(usize, usize)]>,
    fanout: Box<[usize]>,
    /// The surfaces they are on.
    groups: Box<[Group]>,
    /// The writes waiting, in the order they were made: `slots[..waiting]`.
    slots: Box<[Slot]>,
    waiting: AtomicUsize,
    refused: EventCounter,
    /// `refused`'s count as [`LegacyWrites::take_refused`] last took it.
    taken: AtomicU32,
}

/// Its lanes go back to their gates: whatever it had waiting holds no
/// other schedule's writes back.
impl Drop for LegacyWrites {
    fn drop(&mut self) {
        for group in self.groups.iter() {
            group.gate.release(group.lane);
        }
    }
}

/// What became of one attempt at a write.
#[derive(PartialEq)]
enum Attempt {
    Landed,
    Waits,
    Dropped,
}

impl LegacyWrites {
    /// Target `index`, writing `control` on `surface`.
    pub(crate) fn target(
        self: &Arc<Self>,
        index: usize,
        surface: &Surface,
        control: &str,
    ) -> TableTarget {
        TableTarget::Legacy {
            surface: surface.clone(),
            control: control.to_string(),
            writes: self.clone(),
            index,
        }
    }

    /// The controls target `index` reaches.
    fn reached(&self, index: usize) -> &[usize] {
        let (start, end) = self.targets[index];
        &self.fanout[start..end]
    }

    /// Counts the writes that never landed: refused outright, finding no
    /// room to wait, or waiting past the limit; and the ramp starts read
    /// refused.
    pub(crate) fn refused(&self) -> &EventCounter {
        &self.refused
    }

    /// How many writes were counted [`refused`](Self::refused) since the
    /// last call. One thread takes them: the one writing through it.
    pub(crate) fn take_refused(&self) -> u32 {
        let count = self.refused.count();
        count.wrapping_sub(self.taken.swap(count, Ordering::Relaxed))
    }

    /// How many writes wait.
    pub(crate) fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Relaxed)
    }

    /// Writes `value` to every control target `index` reaches, or keeps it
    /// to try again: behind any write to its surface still waiting, from
    /// any schedule, or when the surface is busy.
    pub(crate) fn write(&self, index: usize, value: TableValue) {
        let reached = self.reached(index);
        if reached.is_empty() {
            self.refused.record();
        }
        for &leaf in reached {
            let gate = &self.groups[self.controls[leaf].group].gate;
            if gate.earliest() != IDLE || self.attempt(leaf, value) == Attempt::Waits {
                self.keep(leaf, value);
            }
        }
    }

    /// Reads target `index`'s number where a ramp starts, from the first
    /// control it reaches: the latest number waiting to be written there,
    /// as it will be once it lands (the setter's rounding aside), by this
    /// schedule or else by those in `before` (the ones it replaced, oldest
    /// first); else the control's own. `None` for one holding none, or one
    /// its surface could not read (counted).
    pub(crate) fn read(&self, index: usize, before: &[Arc<LegacyWrites>]) -> Option<f32> {
        let leaf = *self.reached(index).first()?;
        let control = &self.controls[leaf];
        let waiting = self
            .waiting_number(|kept| kept == leaf)
            .or_else(|| (before.iter().rev()).find_map(|writes| writes.waiting_for(control)));
        if waiting.is_some() {
            return waiting;
        }
        match control.surface.get_scheduled(&control.key) {
            Ok(number) => Some(number),
            Err(LegacyRefusal::Unsupported) => None,
            Err(_) => {
                self.refused.record();
                None
            }
        }
    }

    /// The latest number waiting for the control `leaf` says is it.
    fn waiting_number(&self, leaf: impl Fn(usize) -> bool) -> Option<f32> {
        (self.slots[..self.waiting()].iter().rev())
            .map(Slot::load)
            .find_map(|kept| match kept.value {
                TableValue::Number(number) if leaf(kept.index) => Some(number),
                _ => None,
            })
    }

    /// The latest number waiting for another schedule's `control`.
    fn waiting_for(&self, control: &Control) -> Option<f32> {
        self.waiting_number(|leaf| {
            let mine = &self.controls[leaf];
            same(&mine.surface, &control.surface) && mine.key == control.key
        })
    }

    /// Tries every waiting write again, in order: one landing lets the
    /// next to its surface try; one still waiting, here or in another
    /// schedule, holds the later ones back.
    pub(crate) fn retry(&self) {
        let waiting = self.waiting();
        if waiting == 0 {
            return;
        }
        self.hide();
        let mut kept = 0;
        for i in 0..waiting {
            let write = self.slots[i].load();
            let group = &self.groups[self.controls[write.index].group];
            // Those kept already in this pass show their tickets.
            let first = write.ticket < group.gate.earliest();
            if !first || self.attempt(write.index, write.value) == Attempt::Waits {
                self.slots[kept].store(write);
                self.show(group, write.ticket);
                kept += 1;
            }
        }
        self.waiting.store(kept, Ordering::Relaxed);
    }

    /// Ages every waiting write by `samples`, dropping (and counting) the
    /// values that have waited longer than `limit`. A write kept since the
    /// last ageing is not aged yet: however long the block, it is tried
    /// again. An event is never dropped for its wait.
    pub(crate) fn age(&self, samples: u32, limit: u32) {
        self.retain(|control, waited| {
            *waited = match *waited {
                FRESH => 0,
                waited => waited.saturating_add(samples).min(FRESH - 1),
            };
            let keep = control.event || *waited <= limit;
            if !keep {
                self.refused.record();
            }
            keep
        });
    }

    fn attempt(&self, index: usize, value: TableValue) -> Attempt {
        let control = &self.controls[index];
        match control.surface.set_scheduled(&control.key, value) {
            Ok(()) => Attempt::Landed,
            Err(LegacyRefusal::Busy | LegacyRefusal::Full) => Attempt::Waits,
            Err(_) => {
                self.refused.record();
                Attempt::Dropped
            }
        }
    }

    /// Keeps a write to try again: a value in place of its control's latest
    /// waiting one when nothing was kept on its surface since, or, with
    /// no room left for values, when no event was; else after every write
    /// waiting, with a new ticket.
    fn keep(&self, index: usize, value: TableValue) {
        let control = &self.controls[index];
        let group = &self.groups[control.group];
        let waiting = self.waiting();
        let room = match control.event {
            true => RETRY_CAPACITY,
            false => RETRY_CAPACITY - EVENT_ROOM,
        };
        if !control.event {
            let latest = (self.slots[..waiting].iter().rev()).find(|slot| slot.index() == index);
            if let Some(slot) = latest {
                let ticket = slot.load().ticket;
                let crowded = waiting >= room && !group.gate.event_after(ticket);
                if group.gate.is_last(ticket) || crowded {
                    slot.set_value(value);
                    return;
                }
            }
        }
        if waiting >= room {
            self.refused.record();
            return;
        }
        let ticket = group.gate.ticket(control.event);
        self.slots[waiting].store(Kept {
            index,
            value,
            waited: FRESH,
            ticket,
        });
        self.show(group, ticket);
        self.waiting.store(waiting + 1, Ordering::Relaxed);
    }

    /// Drops every waiting write, counted.
    pub(crate) fn drop_waiting(&self) {
        self.retain(|_, _| {
            self.refused.record();
            false
        });
    }

    /// Keeps the waiting writes `keep` says to, in order.
    fn retain(&self, mut keep: impl FnMut(&Control, &mut u32) -> bool) {
        let waiting = self.waiting();
        if waiting == 0 {
            return;
        }
        self.hide();
        let mut kept = 0;
        for i in 0..waiting {
            let mut write = self.slots[i].load();
            let control = &self.controls[write.index];
            if keep(control, &mut write.waited) {
                self.slots[kept].store(write);
                self.show(&self.groups[control.group], write.ticket);
                kept += 1;
            }
        }
        self.waiting.store(kept, Ordering::Relaxed);
    }

    /// Shows the gate `ticket` waiting, unless an earlier one of this
    /// schedule's is shown there already.
    fn show(&self, group: &Group, ticket: u64) {
        if group.gate.shown(group.lane) == IDLE {
            group.gate.show(group.lane, ticket);
        }
    }

    /// Shows every gate nothing of this schedule's waiting.
    fn hide(&self) {
        for group in self.groups.iter() {
            group.gate.show(group.lane, IDLE);
        }
    }
}

#[cfg(test)]
mod tests;
