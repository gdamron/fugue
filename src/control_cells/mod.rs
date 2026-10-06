//! Engine-owned control state: the primitives module controls are built
//! from, so that every ordering argument lives here and not in modules
//! (rules R1–R3 of the control-state discipline).
//!
//! - [`ScalarCell`]: one lock-free value (`f32`, `bool`, `u32`, `i32`, small
//!   enums via [`CellValue`]).
//! - [`SeqCells`]: a fixed group of values that control threads edit
//!   together and the audio thread reads as one consistent [`SeqSnapshot`].
//! - [`EventCounter`]: requests (re-seed, trigger) that must not be lost to
//!   last-writer-wins, observed through an [`EventCursor`].
//! - [`ControlLock`]: the control-side lock multi-cell edits require; never
//!   taken on the audio thread, which debug builds check with an
//!   [`AudioBlockScope`] set by `SignalGraph::process_block`.
//!
//! Everything the audio thread calls is lock-free and allocation-free. No
//! module uses these types yet (melody is the first, FUG-297), hence the
//! `dead_code` and `unused_imports` allowance below; remove it once
//! consumers land.
#![allow(dead_code, unused_imports)]

mod event;
mod guard;
mod scalar;
mod seq;
mod value;

/// The atomics the cells are built on. The loom models compile the same
/// source files against loom's versions instead.
mod sync {
    pub(super) use std::sync::atomic::{AtomicU32, Ordering};
    pub(super) use std::sync::{Mutex, MutexGuard};
    pub(super) use std::thread::yield_now;
}

pub(crate) use event::{EventCounter, EventCursor};
pub(crate) use guard::{AudioBlockScope, ControlGuard, ControlLock};
pub(crate) use scalar::ScalarCell;
pub(crate) use seq::{CellIndex, CellRange, SeqCells, SeqEdit, SeqSnapshot};
pub(crate) use value::CellValue;

#[cfg(test)]
mod tests;
