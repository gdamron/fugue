//! Automation: control writes made on the audio thread itself.
//!
//! A control scheduler runs inside the graph, on the audio thread, so it
//! cannot submit requests (submitting takes the publisher lock). It writes
//! its targets' automation slots instead, and the graph takes each
//! module's slots just before the module processes, applying them through
//! [`apply_declared`] like any request. A development's control has no
//! slot of its own: its writes land in the slots of the inner controls it
//! aliases, taken as each inner module processes.
//!
//! Both ends run on the audio thread, in one block's process order: the
//! graph orders a scheduler before its targets, or runs both sample by
//! sample in a feedback group. So the slots need no ordering of their own
//! and use `Relaxed` throughout; control threads never touch them. A slot
//! holds only the latest write, so writes coalesce per block (per sample in
//! a feedback group), last one wins, exactly as the scheduler's writes to
//! shared controls did before. A refused write is counted, never
//! formatted.
//!
//! A write to a control a higher tier holds (see [`super::hold`]) is
//! dropped and counted, never stored, and its writer told, so a ramp is
//! abandoned and a ramp's next start reads the held value. A schedule entry firing is
//! automation's next event on its control ([`Automation::begin`]): it
//! frees the control first.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use super::cells::{apply_declared, decode, encode, ControlCells};
use super::declare::{integer_domain, DeclKind};
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
    overridden: EventCounter,
}

impl AutomationSlots {
    pub(super) fn new(len: usize) -> Self {
        Self {
            slots: (0..len).map(|_| AtomicU64::new(EMPTY)).collect(),
            pending: AtomicBool::new(false),
            refused: EventCounter::new(),
            overridden: EventCounter::new(),
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

    /// Automation writes dropped so far because a higher tier held their
    /// control.
    pub(crate) fn overridden(&self) -> &EventCounter {
        &self.overridden
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
    /// The range the module clamps a number to (see
    /// [`ControlDecl::clamp`](super::ControlDecl)), applied as it is
    /// written, so a ramp starting from a pending write starts where the
    /// module will be.
    pub(crate) clamp: Option<(f32, f32)>,
    /// A development's control: the inner controls it aliases, in order.
    /// A write lands in each alias's own slot, never in these cells, so
    /// every key reaching an inner control writes the same slot, in the
    /// order the writes are made (a bank-wide key and a per-voice key, last
    /// write wins), and reads it back from there. `None` for a module's own
    /// control.
    pub(crate) aliases: Option<Arc<[Automation]>>,
}

impl Automation {
    /// Writes a number: as is to a number control, to an integer control
    /// only when whole and in its domain (otherwise counted refused). A
    /// clamping control holds it clamped, as its module will; a
    /// development's hands it to each alias as is, to clamp for itself.
    /// Returns whether it was let through: false when a higher tier holds
    /// the control (every alias, for a development), and it was dropped.
    #[inline]
    pub(crate) fn write_number(&self, value: f32) -> bool {
        let value = match self.kind {
            DeclKind::Number { .. } => Some(RtValue::F32(match self.clamp {
                Some((min, max)) => value.max(min).min(max),
                None => value,
            })),
            DeclKind::Integer { min, max } => {
                let (low, high) = integer_domain(min, max, self.clamp);
                let whole = value.fract() == 0.0 && value >= low as f32 && value <= high as f32;
                let held = match (self.clamp, &self.aliases) {
                    (Some(_), None) => (value as i32).clamp(min, max),
                    _ => value as i32,
                };
                whole.then_some(RtValue::I32(held))
            }
            _ => None,
        };
        self.write(value)
    }

    /// Writes a boolean to a boolean control (otherwise counted refused).
    /// Returns whether it was let through, as [`Self::write_number`] does.
    #[inline]
    pub(crate) fn write_bool(&self, value: bool) -> bool {
        let value = (self.kind == DeclKind::Bool).then_some(RtValue::Bool(value));
        self.write(value)
    }

    /// Automation's next scheduled event on this control (a schedule entry
    /// firing on frame `frame` of its module's processing): frees it from
    /// any hold a higher tier took, unless that write was due at the
    /// entry's sample or after it (see `Holds::release`), so the writes
    /// that follow apply. Call it before the entry's first write, or
    /// before reading where its ramp starts.
    #[inline]
    pub(crate) fn begin(&self, frame: usize) {
        self.cells.holds.release(self.index, frame);
        for alias in self.aliases.iter().flat_map(|aliases| aliases.iter()) {
            alias.begin(frame);
        }
    }

    fn write(&self, value: Option<RtValue>) -> bool {
        match (value, &self.aliases) {
            (None, _) => {
                self.cells.automation.refused.record();
                true
            }
            // Checked against the development's own kind first, which every
            // alias takes all of: each then holds it as its own kind does.
            (Some(value), Some(aliases)) => aliases.iter().fold(false, |through, alias| {
                let alias_through = match value {
                    RtValue::F32(number) => alias.write_number(number),
                    RtValue::I32(whole) => alias.write_number(whole as f32),
                    RtValue::Bool(flag) => alias.write_bool(flag),
                    _ => alias.write(None),
                };
                through | alias_through
            }),
            (Some(_), None) if self.cells.holds.holds_off_automation(self.index) => {
                self.cells.automation.overridden.record();
                false
            }
            (Some(value), None) => {
                self.cells.automation.write(self.index, value);
                true
            }
        }
    }

    /// The control's value as automation last left it: its latest write
    /// this block, else what it holds. A ramp starts from here. A
    /// development's control reads its first alias, whichever key wrote it.
    #[inline]
    pub(crate) fn current(&self) -> Option<f32> {
        if let Some(aliases) = &self.aliases {
            return aliases.first()?.current();
        }
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
