//! Plain writes staged per control, applied as their module processes.
//!
//! A request's plain value write is weighed against the holds, and takes
//! its hold, as it falls due, but its setter runs only at a flush: as its
//! module is about to process, or sooner where something needs it applied
//! ([`flush_staged`]). Modules observe controls only as they process, so
//! the heard result is the same; audio-thread readers before then (a
//! ramp's start, a condition's control part) read the staged write
//! ([`ControlCells::current`]). A slot per declared control and a list of
//! the staged ones, in order, are allocated with the module. A second
//! write to a staged control flushes the module first, so every setter
//! runs in due order. Outcomes report at the flush, `Refused` (the hold
//! given back) when the module refuses the write there. Only the thread
//! running the module touches the slots, so `Relaxed` is enough.

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering::Relaxed};

use super::automation::take_automation;
use super::cells::{apply_declared, decode, encode, ControlCells};
use super::declare::{ControlDecl, DeclKind};
use super::hold::{admits_hold, Standing};
use super::pending::{Outcome, Outcomes};
use super::request::{ControlIndex, RequestId, RtValue};
use crate::Module;

/// No write staged: no tag [`encode`] makes.
const EMPTY: u64 = u64::MAX;

/// The request about to apply, as the one recording its outcome tells the
/// graph (see [`Outcomes::applying`]): what a write staged for it reports
/// once flushed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Applying {
    pub(crate) id: RequestId,
    /// Whether it is reported as a timed request.
    pub(crate) timed: bool,
    /// The sample it applies at, reported late when due before it.
    pub(crate) at: u64,
}

/// What a drain's [`Outcomes`] knows of staging: the request about to
/// apply, whether its write was just staged, and how many writes wait.
#[derive(Debug, Default)]
pub(crate) struct Staging {
    applying: Option<Applying>,
    parked: bool,
    staged: usize,
}

impl Staging {
    /// Whether the settle being recorded is that of a write just staged,
    /// which reports at its flush instead. Ends the request's apply.
    pub(super) fn settled(&mut self) -> bool {
        self.applying = None;
        std::mem::take(&mut self.parked)
    }
}

impl Outcomes {
    /// Tells the graph which request applies next, so it may stage the
    /// write: the caller then records its outcome, as always, before any
    /// other.
    pub(crate) fn applying(&mut self, applying: Applying) {
        self.staging.applying = Some(applying);
    }

    /// The request [`Self::applying`] named, once.
    pub(crate) fn take_applying(&mut self) -> Option<Applying> {
        self.staging.applying.take()
    }

    /// How many writes are staged, across the graph.
    pub(crate) fn staged(&self) -> usize {
        self.staging.staged
    }

    fn park(&mut self) {
        self.staging.parked = true;
        self.staging.staged += 1;
    }

    fn flushed(&mut self, id: RequestId, outcome: Outcome, timed: bool) {
        self.staging.staged -= 1;
        self.send(id, false, outcome, None, timed);
    }
}

#[derive(Default)]
struct Slot {
    /// The value its setter takes.
    value: AtomicU64,
    /// The value as its module will hold it, for readers: [`EMPTY`] when
    /// nothing is staged.
    held: AtomicU64,
    id: AtomicU64,
    at: AtomicU64,
    due: AtomicU64,
    timed: AtomicBool,
    /// The hold word the write took, and the one it replaced.
    hold: AtomicU64,
    prev: AtomicU64,
}

/// One module's staged writes: a slot per declared control, and the
/// controls staged, in order.
pub(crate) struct Staged {
    slots: Box<[Slot]>,
    order: Box<[AtomicU16]>,
    len: AtomicUsize,
}

/// A write taken from its slot to flush.
struct Write {
    control: ControlIndex,
    value: RtValue,
    id: RequestId,
    outcome: Outcome,
    timed: bool,
    hold: u64,
    prev: u64,
}

impl Staged {
    pub(super) fn new(len: usize) -> Self {
        let slots: Box<[Slot]> = (0..len)
            .map(|_| Slot {
                held: AtomicU64::new(EMPTY),
                ..Slot::default()
            })
            .collect();
        Self {
            slots,
            order: (0..len).map(|_| AtomicU16::new(0)).collect(),
            len: AtomicUsize::new(0),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len.load(Relaxed) == 0
    }

    /// The write staged on `index`, as its module will hold it.
    #[inline]
    pub(crate) fn held(&self, index: ControlIndex) -> Option<RtValue> {
        let word = self.slots.get(usize::from(index.0))?.held.load(Relaxed);
        (word != EMPTY).then(|| decode(word))
    }

    fn push(&self, control: ControlIndex) {
        let len = self.len.load(Relaxed);
        debug_assert!(len < self.order.len(), "one staged write a control");
        self.order[len].store(control.0, Relaxed);
        self.len.store(len + 1, Relaxed);
    }

    /// Takes the `position`th write staged, emptying its slot.
    fn take(&self, position: usize) -> Option<Write> {
        let control = ControlIndex(self.order.get(position)?.load(Relaxed));
        let slot = self.slots.get(usize::from(control.0))?;
        slot.held.store(EMPTY, Relaxed);
        let (at, due) = (slot.at.load(Relaxed), slot.due.load(Relaxed));
        Some(Write {
            control,
            value: decode(slot.value.load(Relaxed)),
            id: RequestId(slot.id.load(Relaxed)),
            outcome: match due < at {
                true => Outcome::AppliedLate { at, due },
                false => Outcome::Applied { at },
            },
            timed: slot.timed.load(Relaxed),
            hold: slot.hold.load(Relaxed),
            prev: slot.prev.load(Relaxed),
        })
    }
}

impl ControlCells {
    /// The value `index` holds, or the write staged on it: what it will
    /// hold as its module processes. `None` past the table.
    #[inline]
    pub(crate) fn current(&self, index: ControlIndex) -> Option<RtValue> {
        self.staged.held(index).or_else(|| self.load(index))
    }
}

/// `value` as a module holding `decl` keeps it: clamped, a whole number
/// truncated, as automation holds its writes (see
/// [`super::Automation::write_number`]).
fn held(decl: &ControlDecl, value: RtValue) -> RtValue {
    match (decl.kind, value) {
        (DeclKind::Number { .. }, RtValue::F32(value)) => {
            let value = if decl.whole { value.trunc() } else { value };
            RtValue::F32(match decl.clamp {
                Some((min, max)) => value.max(min).min(max),
                None => value,
            })
        }
        (DeclKind::Integer { min, max }, RtValue::I32(value)) if decl.clamp.is_some() => {
            RtValue::I32(value.clamp(min, max))
        }
        _ => value,
    }
}

/// Stages `value`, a plain write of `standing` to `control` of `module`,
/// for `applying`: weighed against the holds, and taking its hold, now,
/// its setter left to [`flush_staged`]. `Ok(false)` when it cannot stage
/// (a control past the table, or one that rewrites others), to apply at
/// once; `Err` is what it lost as. A write to a control already staged
/// flushes the module first. Allocation-, free- and lock-free.
pub(crate) fn stage_write<M: Module + ?Sized>(
    module: &mut M,
    control: ControlIndex,
    value: RtValue,
    standing: Standing,
    applying: Applying,
    outcomes: &mut Outcomes,
) -> Result<bool, Outcome> {
    let Some((table, cells)) = module.declared() else {
        return Ok(false);
    };
    let Some((decl, _)) = table.decl(control) else {
        return Ok(false);
    };
    let staged = cells.staged.held(control).is_some();
    let mut rewrites = false;
    module.for_each_alias(control, &mut |_, _, _| rewrites = true);
    if rewrites || decl.event {
        return Ok(false);
    }
    if staged {
        flush_staged(module, outcomes);
    }
    admits_hold(module, control, standing)?;
    // Automation still waiting was written before this sample.
    take_automation(module);
    let Some((table, cells)) = module.declared() else {
        return Ok(false);
    };
    let (Some((decl, _)), Some(slot)) = (
        table.decl(control),
        cells.staged.slots.get(usize::from(control.0)),
    ) else {
        return Ok(false);
    };
    let (hold, prev) = cells.holds.swap(control, standing);
    slot.value.store(encode(value), Relaxed);
    slot.held.store(encode(held(decl, value)), Relaxed);
    slot.id.store(applying.id.0, Relaxed);
    slot.at.store(applying.at, Relaxed);
    slot.due.store(standing.due, Relaxed);
    slot.timed.store(applying.timed, Relaxed);
    slot.hold.store(hold, Relaxed);
    slot.prev.store(prev, Relaxed);
    cells.staged.push(control);
    outcomes.park();
    Ok(true)
}

/// Runs the setter of every write staged on `module`, in the order staged,
/// and reports each: as it fell due, or `Refused` when the module refuses
/// it, giving back the hold it took. Call it on the thread running the
/// module before it processes, or before anything else applies to it.
/// Allocation-, free- and lock-free; nothing to do when nothing is staged
/// anywhere.
#[inline]
pub(crate) fn flush_staged<M: Module + ?Sized>(module: &mut M, outcomes: &mut Outcomes) {
    if outcomes.staged() == 0 {
        return;
    }
    let len = match module.declared() {
        Some((_, cells)) if !cells.staged.is_empty() => cells.staged.len.load(Relaxed),
        _ => return,
    };
    for position in 0..len {
        let Some(write) = module
            .declared()
            .and_then(|(_, cells)| cells.staged.take(position))
        else {
            continue;
        };
        let outcome = match apply_declared(module, write.control, write.value) {
            Ok(()) => write.outcome,
            Err(refusal) => {
                if let Some((_, cells)) = module.declared() {
                    cells.holds.restore(write.control, write.hold, write.prev);
                }
                Outcome::Refused(refusal)
            }
        };
        outcomes.flushed(write.id, outcome, write.timed);
    }
    if let Some((_, cells)) = module.declared() {
        cells.staged.len.store(0, Relaxed);
    }
}

#[cfg(test)]
mod tests;
