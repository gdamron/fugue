//! A waiting write, held in atomics.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::control_request::table::TableValue;

/// A write's wait before its first retry: not yet aged.
pub(super) const FRESH: u32 = u32::MAX;

/// A write waiting: its control, value, the samples it has waited and its
/// ticket at its surface's gate.
#[derive(Clone, Copy)]
pub(super) struct Kept {
    pub(super) index: usize,
    pub(super) value: TableValue,
    pub(super) waited: u32,
    pub(super) ticket: u64,
}

/// Room for one [`Kept`] write.
#[derive(Default)]
pub(super) struct Slot {
    control: AtomicU32,
    /// The value's bits, and whether it is a boolean (bit 32).
    value: AtomicU64,
    waited: AtomicU32,
    ticket: AtomicU64,
}

impl Slot {
    pub(super) fn load(&self) -> Kept {
        Kept {
            index: self.index(),
            value: bits_value(self.value.load(Ordering::Relaxed)),
            waited: self.waited.load(Ordering::Relaxed),
            ticket: self.ticket.load(Ordering::Relaxed),
        }
    }

    pub(super) fn index(&self) -> usize {
        self.control.load(Ordering::Relaxed) as usize
    }

    pub(super) fn store(&self, kept: Kept) {
        self.control.store(kept.index as u32, Ordering::Relaxed);
        self.set_value(kept.value);
        self.waited.store(kept.waited, Ordering::Relaxed);
        self.ticket.store(kept.ticket, Ordering::Relaxed);
    }

    pub(super) fn set_value(&self, value: TableValue) {
        self.value.store(value_bits(value), Ordering::Relaxed);
    }
}

/// A value as bits an atomic holds: a number's, or a boolean's with bit 32.
pub(super) fn value_bits(value: TableValue) -> u64 {
    match value {
        TableValue::Number(number) => number.to_bits() as u64,
        TableValue::Bool(flag) => 1 << 32 | flag as u64,
    }
}

pub(super) fn bits_value(bits: u64) -> TableValue {
    match bits >> 32 {
        0 => TableValue::Number(f32::from_bits(bits as u32)),
        _ => TableValue::Bool(bits & 1 == 1),
    }
}
