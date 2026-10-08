//! Automation: control writes made on the audio thread itself.
//!
//! A control scheduler runs inside the graph, on the audio thread, so it
//! cannot submit requests (submitting takes the publisher lock). It writes
//! its targets' automation slots instead, and the graph takes each
//! module's slots just before the module processes, applying them through
//! [`apply_declared`] like any request.
//!
//! Both ends run on the audio thread, in one block's process order: the
//! graph orders a scheduler before its targets, or runs both sample by
//! sample in a feedback group. So the slots need no ordering of their own
//! and use `Relaxed` throughout; control threads never touch them. A slot
//! holds only the latest write, so writes coalesce per block (per sample in
//! a feedback group), last one wins, exactly as the scheduler's writes to
//! shared controls did before. A refused write is counted, never
//! formatted.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use super::cells::{apply_declared, decode, encode, ControlCells};
use super::declare::DeclKind;
use super::event::EventCounter;
use super::request::{ControlIndex, RtValue};
use crate::Module;

/// An empty slot: no tag [`encode`] makes.
const EMPTY: u64 = u64::MAX;

/// One module's automation slots, one per declared control.
pub(crate) struct AutomationSlots {
    slots: Box<[AtomicU64]>,
    pending: AtomicBool,
    refused: EventCounter,
}

impl AutomationSlots {
    pub(super) fn new(len: usize) -> Self {
        Self {
            slots: (0..len).map(|_| AtomicU64::new(EMPTY)).collect(),
            pending: AtomicBool::new(false),
            refused: EventCounter::new(),
        }
    }

    fn write(&self, index: ControlIndex, value: RtValue) {
        if let Some(slot) = self.slots.get(usize::from(index.0)) {
            slot.store(encode(value), Ordering::Relaxed);
            self.pending.store(true, Ordering::Relaxed);
        }
    }

    fn written(&self, index: ControlIndex) -> Option<RtValue> {
        let word = self
            .slots
            .get(usize::from(index.0))?
            .load(Ordering::Relaxed);
        (word != EMPTY).then(|| decode(word))
    }

    /// Automation writes refused so far: by the module, or as a value its
    /// control cannot hold (a fraction for an integer).
    pub(crate) fn refused(&self) -> &EventCounter {
        &self.refused
    }
}

/// Applies every automation write waiting for `module`, in control order,
/// counting refusals. Call it on the audio thread just before the module
/// processes. Allocation-, free- and lock-free; one `Relaxed` load when
/// nothing is waiting.
#[inline]
pub(crate) fn take_automation<M: Module + ?Sized>(module: &mut M) {
    let len = match module.declared() {
        Some((_, cells)) if cells.automation.pending.load(Ordering::Relaxed) => {
            cells.automation.pending.store(false, Ordering::Relaxed);
            cells.len()
        }
        _ => return,
    };
    for index in 0..len {
        let index = ControlIndex(index as u16);
        let Some((_, cells)) = module.declared() else {
            return;
        };
        let Some(slot) = cells.automation.slots.get(usize::from(index.0)) else {
            continue;
        };
        let word = slot.swap(EMPTY, Ordering::Relaxed);
        if word != EMPTY && apply_declared(module, index, decode(word)).is_err() {
            if let Some((_, cells)) = module.declared() {
                cells.automation.refused.record();
            }
        }
    }
}

/// One declared control as automation writes it: resolved on a control
/// thread, written from the audio thread.
#[derive(Clone)]
pub(crate) struct Automation {
    pub(crate) cells: Arc<ControlCells>,
    pub(crate) index: ControlIndex,
    pub(crate) kind: DeclKind,
}

impl Automation {
    /// Writes a number: as is to a number control, to an integer control
    /// only when whole and in range (otherwise counted refused).
    #[inline]
    pub(crate) fn write_number(&self, value: f32) {
        let value = match self.kind {
            DeclKind::Number { .. } => Some(RtValue::F32(value)),
            DeclKind::Integer { min, max } => {
                let whole = value.fract() == 0.0 && value >= min as f32 && value <= max as f32;
                whole.then_some(RtValue::I32(value as i32))
            }
            _ => None,
        };
        self.write(value);
    }

    /// Writes a boolean to a boolean control (otherwise counted refused).
    #[inline]
    pub(crate) fn write_bool(&self, value: bool) {
        let value = (self.kind == DeclKind::Bool).then_some(RtValue::Bool(value));
        self.write(value);
    }

    fn write(&self, value: Option<RtValue>) {
        match value {
            Some(value) => self.cells.automation.write(self.index, value),
            None => self.cells.automation.refused.record(),
        }
    }

    /// The control's value as automation last left it: its latest write
    /// this block, else what it holds. A ramp starts from here.
    #[inline]
    pub(crate) fn current(&self) -> Option<f32> {
        let value = self
            .cells
            .automation
            .written(self.index)
            .or_else(|| self.cells.load(self.index))?;
        match value {
            RtValue::F32(value) => Some(value),
            RtValue::I32(value) => Some(value as f32),
            _ => None,
        }
    }
}
