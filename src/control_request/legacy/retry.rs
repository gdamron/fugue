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
//! - **Bound.** At most [`RETRY_CAPACITY`] writes wait, each at most the
//!   scheduler's limit (10 ms of samples). A write past either is dropped
//!   and counted in [`LegacyWrites::refused`], as is one its surface
//!   refuses outright (a control it has not got, a value it does not take).
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

/// The legacy controls one schedule writes, collected on a control thread
/// as it resolves.
#[derive(Default)]
pub(crate) struct LegacyControls(Vec<(Surface, String)>);

impl LegacyControls {
    /// The index of `key` on `surface`: the same for every entry writing it.
    pub(crate) fn index(&mut self, surface: &Surface, key: &str) -> usize {
        let same = |(other, other_key): &(Surface, String)| {
            std::ptr::addr_eq(Arc::as_ptr(surface), Arc::as_ptr(other)) && other_key == key
        };
        self.0.iter().position(same).unwrap_or_else(|| {
            self.0.push((surface.clone(), key.to_string()));
            self.0.len() - 1
        })
    }

    /// The schedule's writes, with room for those that wait. Allocates.
    pub(crate) fn build(self) -> Arc<LegacyWrites> {
        let mut surfaces: Vec<*const ()> = Vec::new();
        let controls = (self.0.into_iter())
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
            slots: (0..RETRY_CAPACITY).map(|_| Slot::default()).collect(),
            waiting: AtomicUsize::new(0),
            refused: EventCounter::new(),
        })
    }
}

/// A schedule's writes to its legacy controls, and those waiting to land
/// (see the module docs). Allocation-, free- and lock-free once built.
pub(crate) struct LegacyWrites {
    controls: Box<[Control]>,
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
    /// The target writing control `index`.
    pub(crate) fn target(self: &Arc<Self>, index: usize) -> TableTarget {
        let control = &self.controls[index];
        TableTarget::Legacy {
            surface: control.surface.clone(),
            control: control.key.clone(),
            writes: self.clone(),
            index,
        }
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

    /// Writes `value` to control `index`, or keeps it to try again: behind
    /// any write to its surface still waiting, or when the surface is busy.
    pub(crate) fn write(&self, index: usize, value: TableValue) {
        let group = self.controls[index].group;
        let behind = self.slots[..self.waiting()]
            .iter()
            .any(|slot| self.controls[slot.load().0].group == group);
        if behind || self.attempt(index, value) == Attempt::Waits {
            self.keep(index, value);
        }
    }

    /// Reads control `index`'s number where a ramp starts: `None` for one
    /// holding none, or one its surface could not read (counted).
    pub(crate) fn read(&self, index: usize) -> Option<f32> {
        let control = &self.controls[index];
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
    /// that have waited longer than `limit`.
    pub(crate) fn age(&self, samples: u32, limit: u32) {
        self.retain(|_, waited| {
            *waited = waited.saturating_add(samples);
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
        slot.store(index, value, 0);
        self.waiting.store(waiting + 1, Ordering::Relaxed);
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
