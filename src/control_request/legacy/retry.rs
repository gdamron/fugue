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
//! - **Order.** A surface's writes land in the order they were made: while
//!   one waits, every later write to that surface waits behind it, so a
//!   `note_off` never lands before its `note_on`.
//! - **Coalescing.** A value's waiting write gives way to a newer one to the
//!   same control, which lands as the latest would have, so a ramp waiting
//!   takes one slot. An event ([`crate::ControlSurface::scheduled_event`]:
//!   a note, a play) is never merged: each waits its turn.
//! - **Bound.** At most [`RETRY_CAPACITY`] writes wait. Each is tried again
//!   at least once, and then for at most the scheduler's limit (10 ms of
//!   samples). A write past either is dropped and counted in
//!   [`LegacyWrites::refused`], as is one its surface refuses outright (a
//!   control it has not got, a value it does not take).
//! - **Fan-out.** A development's control reaching several aliases (see
//!   [`crate::ControlSurface::scheduled_aliases`]) is written alias by
//!   alias, so one alias busy waits alone: the others take the write once.
//! - **Adoption.** A new schedule takes over the old one's waiting writes
//!   to the controls it still reaches, ahead of its own (see
//!   [`LegacyWrites::take_waiting`]).
//!
//! Its state is atomics, so it is `Sync`, but one thread at a time writes
//! through it: the audio thread, or an offline render's.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use super::LegacyRefusal;
use crate::control_request::table::{TableTarget, TableValue};
use crate::control_request::EventCounter;
use crate::ControlSurface;

/// The most writes one schedule keeps waiting.
pub(crate) const RETRY_CAPACITY: usize = 32;

type Surface = Arc<dyn ControlSurface + Send + Sync>;

/// One legacy control a schedule writes, and the surface it is on.
struct Control {
    surface: Surface,
    key: String,
    /// The same for every control on one surface: its writes keep order.
    group: usize,
}

/// A write's wait before its first retry: not yet aged.
const FRESH: u32 = u32::MAX;

/// A write waiting: its control, value and the samples it has waited.
#[derive(Default)]
struct Slot {
    control: AtomicU32,
    /// The value's bits, and whether it is a boolean (bit 32).
    value: AtomicU64,
    waited: AtomicU32,
}

impl Slot {
    fn load(&self) -> (usize, TableValue, u32) {
        let bits = self.value.load(Ordering::Relaxed);
        let value = match bits >> 32 {
            0 => TableValue::Number(f32::from_bits(bits as u32)),
            _ => TableValue::Bool(bits & 1 == 1),
        };
        let control = self.control.load(Ordering::Relaxed) as usize;
        (control, value, self.waited.load(Ordering::Relaxed))
    }

    fn store(&self, control: usize, value: TableValue, waited: u32) {
        let bits = match value {
            TableValue::Number(number) => number.to_bits() as u64,
            TableValue::Bool(flag) => 1 << 32 | flag as u64,
        };
        self.control.store(control as u32, Ordering::Relaxed);
        self.value.store(bits, Ordering::Relaxed);
        self.waited.store(waited, Ordering::Relaxed);
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

    /// The schedule's writes, with room for those that wait. Allocates.
    pub(crate) fn build(self) -> Arc<LegacyWrites> {
        let mut surfaces: Vec<*const ()> = Vec::new();
        let controls = (self.leaves.into_iter())
            .map(|(surface, key)| {
                let at = Arc::as_ptr(&surface) as *const ();
                let group = surfaces.iter().position(|&seen| seen == at);
                let group = group.unwrap_or_else(|| {
                    surfaces.push(at);
                    surfaces.len() - 1
                });
                Control {
                    surface,
                    key,
                    group,
                }
            })
            .collect();
        Arc::new(LegacyWrites {
            controls,
            targets: self.targets.into(),
            fanout: self.fanout.into(),
            slots: (0..RETRY_CAPACITY).map(|_| Slot::default()).collect(),
            waiting: AtomicUsize::new(0),
            refused: EventCounter::new(),
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
    /// The writes waiting, in the order they were made: `slots[..waiting]`.
    slots: Box<[Slot]>,
    waiting: AtomicUsize,
    refused: EventCounter,
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

    /// How many writes wait.
    pub(crate) fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Relaxed)
    }

    /// Writes `value` to every control target `index` reaches, or keeps it
    /// to try again: behind any write to its surface still waiting, or when
    /// the surface is busy.
    pub(crate) fn write(&self, index: usize, value: TableValue) {
        let reached = self.reached(index);
        if reached.is_empty() {
            self.refused.record();
        }
        for &leaf in reached {
            self.write_leaf(leaf, value);
        }
    }

    fn write_leaf(&self, index: usize, value: TableValue) {
        let group = self.controls[index].group;
        let behind = self.slots[..self.waiting()]
            .iter()
            .any(|slot| self.controls[slot.load().0].group == group);
        if behind || self.attempt(index, value) == Attempt::Waits {
            self.keep(index, value);
        }
    }

    /// Reads target `index`'s number where a ramp starts, from the first
    /// control it reaches: the latest number waiting to be written there,
    /// as it will be once it lands (the setter's rounding aside), else the
    /// control's own. `None` for one holding none, or one its surface could
    /// not read (counted).
    pub(crate) fn read(&self, index: usize) -> Option<f32> {
        let leaf = *self.reached(index).first()?;
        let waiting = (self.slots[..self.waiting()].iter().rev())
            .map(Slot::load)
            .find_map(|(other, value, _)| match value {
                TableValue::Number(number) if other == leaf => Some(number),
                _ => None,
            });
        if waiting.is_some() {
            return waiting;
        }
        let control = &self.controls[leaf];
        match control.surface.get_scheduled(&control.key) {
            Ok(number) => Some(number),
            Err(LegacyRefusal::Unsupported) => None,
            Err(_) => {
                self.refused.record();
                None
            }
        }
    }

    /// Tries every waiting write again, in order: one landing lets the
    /// next to its surface try; one still waiting holds the rest back.
    pub(crate) fn retry(&self) {
        let waiting = self.waiting();
        let mut kept = 0;
        for i in 0..waiting {
            let (index, value, waited) = self.slots[i].load();
            let group = self.controls[index].group;
            let held =
                (self.slots[..kept].iter()).any(|slot| self.controls[slot.load().0].group == group);
            if held || self.attempt(index, value) == Attempt::Waits {
                self.slots[kept].store(index, value, waited);
                kept += 1;
            }
        }
        self.waiting.store(kept, Ordering::Relaxed);
    }

    /// Ages every waiting write by `samples`, dropping (and counting) those
    /// that have waited longer than `limit`. A write kept since the last
    /// ageing is not aged yet: however long the block, it is tried again.
    pub(crate) fn age(&self, samples: u32, limit: u32) {
        self.retain(|_, waited| {
            *waited = match *waited {
                FRESH => 0,
                waited => waited.saturating_add(samples).min(FRESH - 1),
            };
            let keep = *waited <= limit;
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

    /// Keeps a write to try again, after the latest waiting value of its
    /// control gives way to it (an event's never does).
    fn keep(&self, index: usize, value: TableValue) {
        let control = &self.controls[index];
        if !control.surface.scheduled_event(&control.key) {
            self.retain(|other, _| other != index);
        }
        let waiting = self.waiting();
        let Some(slot) = self.slots.get(waiting) else {
            self.refused.record();
            return;
        };
        slot.store(index, value, FRESH);
        self.waiting.store(waiting + 1, Ordering::Relaxed);
    }

    /// Takes over `old`'s waiting writes, as the schedule replacing it:
    /// those to a control this one reaches go ahead of its own, in order;
    /// the rest are dropped and counted, as are any finding no room.
    pub(crate) fn take_waiting(&self, old: &LegacyWrites) {
        let leaf = |slot: &Slot| {
            let control = &old.controls[slot.load().0];
            (self.controls.iter())
                .position(|ours| same(&ours.surface, &control.surface) && ours.key == control.key)
        };
        let theirs = &old.slots[..old.waiting()];
        let carried = theirs.iter().filter(|slot| leaf(slot).is_some()).count();
        let lost = theirs.len() - carried;
        let mine = self.waiting();
        let kept = mine.min(RETRY_CAPACITY - carried);
        for i in (0..kept).rev() {
            let (index, value, waited) = self.slots[i].load();
            self.slots[carried + i].store(index, value, waited);
        }
        let mut at = 0;
        for slot in theirs {
            if let Some(index) = leaf(slot) {
                let (_, value, waited) = slot.load();
                self.slots[at].store(index, value, waited);
                at += 1;
            }
        }
        self.waiting.store(carried + kept, Ordering::Relaxed);
        old.waiting.store(0, Ordering::Relaxed);
        for _ in 0..lost + mine - kept {
            self.refused.record();
        }
    }

    /// Drops every waiting write, counted: its schedule is replaced by one
    /// writing no legacy control.
    pub(crate) fn drop_waiting(&self) {
        self.retain(|_, _| {
            self.refused.record();
            false
        });
    }

    /// Keeps the waiting writes `keep` says to, in order.
    fn retain(&self, mut keep: impl FnMut(usize, &mut u32) -> bool) {
        let waiting = self.waiting();
        let mut kept = 0;
        for i in 0..waiting {
            let (index, value, mut waited) = self.slots[i].load();
            if keep(index, &mut waited) {
                self.slots[kept].store(index, value, waited);
                kept += 1;
            }
        }
        self.waiting.store(kept, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests;
