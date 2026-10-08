//! Applied control state, published for control threads to read back.

use std::sync::atomic::{AtomicU64, Ordering};

use super::pending::Refusal;
use super::request::{ControlIndex, RtValue};
use crate::Module;

/// The value each of a module's declared controls holds, published for
/// control threads to read back (`get_control`, describe).
///
/// # Writers (R1)
///
/// Before the module runs, the control thread building it is the only
/// writer: its initial state, and writes made before it is bound. Once it
/// runs, only the thread running it writes, through [`apply_declared`]:
/// the audio thread, or an offline render under its graph's lock. The
/// module's publication to the audio thread (the mailbox put and take)
/// orders every earlier store before the audio thread's first.
///
/// # What a reader sees
///
/// One cell holds one control, its kind and bits stored together in one
/// word, so a read is always a value the control held, never a torn one.
/// Stores are `Release` and loads `Acquire`, so a reader that sees a value
/// also sees every cell the same writer published before it. Cells are
/// otherwise independent: two controls read one after the other may straddle
/// an apply. State that must be read as one across several controls needs
/// a snapshot of its own (a seqlock, with the first module that has such
/// state: melody's scale).
pub(crate) struct ControlCells {
    cells: Box<[AtomicU64]>,
}

impl ControlCells {
    /// Cells holding `values`, in control index order. Allocates: call it on
    /// a control thread, as the module is built.
    pub(crate) fn new(values: impl IntoIterator<Item = RtValue>) -> Self {
        let cells = values
            .into_iter()
            .map(|value| AtomicU64::new(encode(value)))
            .collect();
        Self { cells }
    }

    pub(crate) fn len(&self) -> usize {
        self.cells.len()
    }

    /// Publishes `value` as the value `index` holds. Wait-free, allocation-
    /// and lock-free; an index past the table is ignored.
    #[inline]
    pub(crate) fn publish(&self, index: ControlIndex, value: RtValue) {
        if let Some(cell) = self.cells.get(usize::from(index.0)) {
            cell.store(encode(value), Ordering::Release);
        }
    }

    /// The value `index` holds, or `None` past the table.
    #[inline]
    pub(crate) fn load(&self, index: ControlIndex) -> Option<RtValue> {
        let cell = self.cells.get(usize::from(index.0))?;
        Some(decode(cell.load(Ordering::Acquire)))
    }
}

/// Applies `value` to declared control `control` of `module` and publishes
/// what the control then holds. The one way a declared control changes once
/// its module runs: allocation-, free- and lock-free as long as the
/// module's [`apply`](Module::apply) is.
#[inline]
pub(crate) fn apply_declared<M: Module + ?Sized>(
    module: &mut M,
    control: ControlIndex,
    value: RtValue,
) -> Result<(), Refusal> {
    let applied = module.apply(control, value)?;
    if let Some((_, cells)) = module.declared() {
        cells.publish(control, applied);
    }
    Ok(())
}

const F32: u64 = 0;
const U32: u64 = 1;
const I32: u64 = 2;
const BOOL: u64 = 3;

#[inline]
fn encode(value: RtValue) -> u64 {
    let (tag, bits) = match value {
        RtValue::F32(value) => (F32, value.to_bits()),
        RtValue::U32(value) => (U32, value),
        RtValue::I32(value) => (I32, value as u32),
        RtValue::Bool(value) => (BOOL, u32::from(value)),
    };
    (tag << 32) | u64::from(bits)
}

#[inline]
fn decode(word: u64) -> RtValue {
    let bits = word as u32;
    match word >> 32 {
        F32 => RtValue::F32(f32::from_bits(bits)),
        U32 => RtValue::U32(bits),
        I32 => RtValue::I32(bits as i32),
        _ => RtValue::Bool(bits != 0),
    }
}

#[cfg(test)]
mod tests;
