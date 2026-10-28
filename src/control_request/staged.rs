//! Plain writes staged per control: a request's write takes its hold as it
//! falls due, and its setter runs at a flush ([`flush_staged`]), as its
//! module is about to process or before something needs it applied. The
//! heard result is the same; a ramp starting from the control reads the
//! staged value ([`ControlCells::current`]). Slots are allocated with the
//! module, and only the thread running it touches them (`Relaxed`).
//!
//! One write stands on a control at a sample ([`contest`]): of the requests
//! reaching it there, staged or applied at once, the one due last, then of
//! the higher rank, then received last, as if each had applied on time.
//! Behind an edit held to a later block, a write is weighed so against one
//! applied since the edit was first due too ([`Outcomes::edit_held`]).
//! One that loses reports so at once, and a staged one's setter never runs.
//! A write applied at once runs after the weaker writes staged on its
//! module and before the stronger ([`flush_staged_before`]).

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering::Relaxed};

use super::automation::take_automation;
use super::cells::{apply_declared, decode, encode, ControlCells};
use super::chains::Chains;
use super::declare::{ControlDecl, DeclKind};
use super::hold::{admits_hold, rank_of, Rank, Standing};
use super::pending::{Outcome, Outcomes};
use super::request::{ControlIndex, RequestId, RtValue};
use crate::control_request::declare::ControlTable;
use crate::Module;

/// No write staged: no tag [`encode`] makes.
const EMPTY: u64 = u64::MAX;

/// The request about to apply (see [`Outcomes::applying`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Applying {
    pub(crate) id: RequestId,
    /// Whether it is reported as a timed request.
    pub(crate) timed: bool,
    /// The sample it applies at, reported late when due before it.
    pub(crate) at: u64,
    /// The sample it was due at.
    pub(crate) due: u64,
}

/// Where a write applied at once runs among the writes staged on its
/// module at its sample ([`flush_staged_before`]): after those weaker than
/// `key` (by due sample, rank and receipt), before the stronger.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Before {
    at: u64,
    key: (u64, Rank, u64),
}

impl Before {
    /// For a write of `standing`, weighed as [`contest`] weighs it.
    pub(crate) fn write(standing: Standing, applying: Applying) -> Self {
        let key = (standing.due, standing.rank, applying.id.0);
        Self {
            at: applying.at,
            key,
        }
    }

    /// For an event, which holds nothing: after every write staged at its
    /// sample due no later than it, before those due after it, so it reads
    /// the values in force at its own due sample.
    pub(crate) fn event(applying: Applying) -> Self {
        let key = (applying.due, Rank::MAX, u64::MAX);
        Self {
            at: applying.at,
            key,
        }
    }

    /// The sample it applies at.
    pub(crate) fn at(self) -> u64 {
        self.at
    }
}

/// The request applying, whether its write was just staged, how many wait,
/// and since when an edit held to a later block waited.
#[derive(Debug, Default)]
pub(crate) struct Staging {
    applying: Option<Applying>,
    parked: bool,
    staged: usize,
    held: Option<u64>,
    since: Option<u64>,
}

impl Staging {
    /// Whether the settle being recorded is a staged write's, sent at its flush.
    pub(super) fn settled(&mut self) -> bool {
        self.applying = None;
        std::mem::take(&mut self.parked)
    }
}

impl Outcomes {
    /// Names the request applying next, so the graph may stage its write.
    pub(crate) fn applying(&mut self, applying: Applying) {
        self.staging.applying = Some(applying);
    }

    /// The request [`Self::applying`] named, once.
    pub(crate) fn take_applying(&mut self) -> Option<Applying> {
        self.staging.applying.take()
    }

    /// Notes that an edit due at `now` waits to install, from the first
    /// such sample: an edit held behind ones installing now (past the
    /// installs a block takes) waits from when the first did.
    pub(crate) fn edit_held(&mut self, now: u64) {
        let first = self.staging.since.unwrap_or(now);
        self.staging.held.get_or_insert(first);
    }

    /// As the edit installs: until [`Self::weighed`], the requests queued
    /// behind it are weighed as if it had installed when first due, against
    /// the writes standing since ([`contest`]). Only the first edit to
    /// install was held; one installing behind it keeps that sample.
    pub(crate) fn edit_installs(&mut self) {
        if let Some(held) = self.staging.held.take() {
            self.staging.since = Some(held);
        }
    }

    /// Ends what [`Self::edit_installs`] began, once those requests applied.
    pub(crate) fn weighed(&mut self) {
        self.staging.since = None;
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

/// A control's staged write, and the write standing on it at sample `at`:
/// the one staged, else the last applied at once (none while `hold` is 0).
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

impl Slot {
    /// The write standing here at `at`, by what decides between two: its
    /// due sample, rank and receipt.
    fn standing_at(&self, at: u64) -> Option<(u64, Rank, u64)> {
        let rank = rank_of(self.hold.load(Relaxed))?;
        let due = self.due.load(Relaxed);
        (self.at.load(Relaxed) == at).then(|| (due, rank, self.id.load(Relaxed)))
    }

    /// The write standing here since sample `since`, by what decides
    /// between two: its due sample, rank and receipt.
    fn standing_since(&self, since: u64) -> Option<(u64, Rank, u64)> {
        let rank = rank_of(self.hold.load(Relaxed))?;
        let due = self.due.load(Relaxed);
        (self.at.load(Relaxed) >= since).then(|| (due, rank, self.id.load(Relaxed)))
    }

    /// Settles the write staged here at `at` as losing to one of
    /// `winner`'s, its setter never run; the winner takes its hold over,
    /// as it would from one applied. Whether one was staged.
    fn give_way(&self, at: u64, winner: Rank, outcomes: &mut Outcomes) -> bool {
        let staged = self.held.load(Relaxed) != EMPTY && self.at.load(Relaxed) == at;
        let (Some(rank), true) = (rank_of(self.hold.load(Relaxed)), staged) else {
            return false;
        };
        self.held.store(EMPTY, Relaxed);
        self.hold.store(0, Relaxed);
        let id = RequestId(self.id.load(Relaxed));
        outcomes.flushed(id, rank.loses_to(winner), self.timed.load(Relaxed));
        true
    }
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

    /// Whether every write still staged is for sample `at` (none when
    /// `at` is none).
    pub(crate) fn waits_only_at(&self, at: Option<u64>) -> bool {
        let len = self.len.load(Relaxed);
        self.order.iter().take(len).all(|control| {
            let slot = self.slots.get(usize::from(control.load(Relaxed)));
            slot.is_none_or(|slot| {
                slot.held.load(Relaxed) == EMPTY || Some(slot.at.load(Relaxed)) == at
            })
        })
    }

    /// Whether a condition waits on a write staged here.
    fn watched(&self, chains: &Chains) -> bool {
        let len = self.len.load(Relaxed);
        self.order.iter().take(len).any(|control| {
            let slot = self.slots.get(usize::from(control.load(Relaxed)));
            slot.is_some_and(|slot| {
                slot.held.load(Relaxed) != EMPTY
                    && chains.watching(RequestId(slot.id.load(Relaxed)))
            })
        })
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

    /// Whether the `position`th write staged runs after `than`, at its
    /// sample, moving it to place `kept` if so.
    fn keeps(&self, position: usize, kept: usize, than: Option<Before>) -> bool {
        let (Some(than), Some(control)) = (than, self.order.get(position)) else {
            return false;
        };
        let control = control.load(Relaxed);
        // One that gave way stands no more (its hold is 0).
        let stronger = (self.slots.get(usize::from(control)))
            .and_then(|slot| slot.standing_at(than.at))
            .is_some_and(|theirs| theirs > than.key);
        if stronger {
            self.order[kept].store(control, Relaxed);
        }
        stronger
    }

    /// Takes the `position`th write staged, emptying its slot (none for
    /// one that gave way).
    fn take(&self, position: usize) -> Option<Write> {
        let control = ControlIndex(self.order.get(position)?.load(Relaxed));
        let slot = self.slots.get(usize::from(control.0))?;
        if slot.held.swap(EMPTY, Relaxed) == EMPTY {
            return None;
        }
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
    /// The write staged on `index`, else the value it holds.
    #[inline]
    pub(crate) fn current(&self, index: ControlIndex) -> Option<RtValue> {
        self.staged.held(index).or_else(|| self.load(index))
    }
}

/// `value` clamped (and truncated) as `decl`'s module keeps it.
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

/// Weighs a write of `standing`, applying as `applying` names, against the
/// write standing on `control` at the same sample (see the module docs),
/// then against the holds as [`admits_hold`] does: `Err` what it loses
/// as. Once it wins, a write staged there gives way: `Ok(true)` when one
/// did. Allocation- and lock-free.
pub(crate) fn contest<M: Module + ?Sized>(
    module: &M,
    control: ControlIndex,
    standing: Standing,
    applying: Applying,
    outcomes: &mut Outcomes,
) -> Result<bool, Outcome> {
    let Some(slot) = module
        .declared()
        .and_then(|(_, cells)| cells.staged.slots.get(usize::from(control.0)))
    else {
        return Ok(false);
    };
    let mine = (standing.due, standing.rank, applying.id.0);
    let since = outcomes
        .staging
        .since
        .unwrap_or(applying.at)
        .min(applying.at);
    if let Some(theirs) = slot.standing_since(since).filter(|theirs| mine < *theirs) {
        return Err(standing.rank.loses_to(theirs.1));
    }
    admits_hold(module, control, standing)?;
    Ok(slot.give_way(applying.at, standing.rank, outcomes))
}

/// Records that a write of `standing`, applying as `applying` names,
/// applied at once to `control` and every control it reaches: it stands
/// there for [`contest`], but for a stronger write still staged there to
/// run after it. Allocation- and lock-free.
pub(crate) fn stand<M: Module + ?Sized>(
    module: &M,
    control: ControlIndex,
    standing: Standing,
    applying: Applying,
) {
    let mut stand = |_: &ControlTable, cells: &ControlCells, index: ControlIndex| {
        let slot = cells.staged.slots.get(usize::from(index.0));
        if let Some(slot) = slot.filter(|slot| slot.held.load(Relaxed) == EMPTY) {
            slot.at.store(applying.at, Relaxed);
            slot.due.store(standing.due, Relaxed);
            slot.id.store(applying.id.0, Relaxed);
            slot.hold.store(standing.word(), Relaxed);
        }
    };
    if let Some((table, cells)) = module.declared() {
        stand(table, cells, control);
    }
    module.for_each_alias(control, &mut stand);
}

/// Stages a plain write, taking its hold now: `Ok(false)` when it must apply
/// at once (an event, or a control that rewrites others), `Err` what it
/// lost as ([`contest`]). One staged at an earlier sample flushes first.
/// Allocation- and lock-free.
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
    let (Some((decl, _)), Some(slot)) = (
        table.decl(control),
        cells.staged.slots.get(usize::from(control.0)),
    ) else {
        return Ok(false);
    };
    let earlier = slot.held.load(Relaxed) != EMPTY && slot.at.load(Relaxed) != applying.at;
    let mut rewrites = false;
    module.for_each_alias(control, &mut |_, _, _| rewrites = true);
    if rewrites || decl.event {
        return Ok(false);
    }
    if earlier {
        flush_staged(module, outcomes);
    }
    let listed = contest(&*module, control, standing, applying, outcomes)?;
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
    // In the place of one that gave way, as if neither had applied.
    let prev = if listed {
        slot.prev.load(Relaxed)
    } else {
        prev
    };
    slot.value.store(encode(value), Relaxed);
    slot.held.store(encode(held(decl, value)), Relaxed);
    slot.id.store(applying.id.0, Relaxed);
    slot.at.store(applying.at, Relaxed);
    slot.due.store(standing.due, Relaxed);
    slot.timed.store(applying.timed, Relaxed);
    slot.hold.store(hold, Relaxed);
    slot.prev.store(prev, Relaxed);
    // One that gave way here keeps its place in the order.
    if !listed {
        cells.staged.push(control);
    }
    outcomes.park();
    Ok(true)
}

/// Runs every setter staged on `module`, in order, and reports each; one
/// refused gives back its hold, and stands no more. Allocation- and
/// lock-free.
#[inline]
pub(crate) fn flush_staged<M: Module + ?Sized>(module: &mut M, outcomes: &mut Outcomes) {
    flush_staged_before(module, None, outcomes);
}

/// [`flush_staged`], once a condition waits on a write staged on `module`
/// (one staged before the condition started): it learns the write applied
/// at its sample, as a write it waits on applies at once. Allocation- and
/// lock-free.
pub(crate) fn flush_watched<M: Module + ?Sized>(module: &mut M, outcomes: &mut Outcomes) {
    let watched = match module.declared() {
        Some((_, cells)) => cells.staged.watched(&outcomes.chains),
        None => false,
    };
    if watched {
        flush_staged(module, outcomes);
    }
}

/// [`flush_staged`], but a write staged at `than`'s sample that runs after
/// it ([`Before`]) stays staged. Allocation- and lock-free.
pub(crate) fn flush_staged_before<M: Module + ?Sized>(
    module: &mut M,
    than: Option<Before>,
    outcomes: &mut Outcomes,
) {
    if outcomes.staged() == 0 {
        return;
    }
    let len = match module.declared() {
        Some((_, cells)) if !cells.staged.is_empty() => cells.staged.len.load(Relaxed),
        _ => return,
    };
    let mut kept = 0;
    for position in 0..len {
        let Some((_, cells)) = module.declared() else {
            return;
        };
        if cells.staged.keeps(position, kept, than) {
            kept += 1;
            continue;
        }
        let Some(write) = cells.staged.take(position) else {
            continue;
        };
        let outcome = match apply_declared(module, write.control, write.value) {
            Ok(()) => write.outcome,
            Err(refusal) => {
                if let Some((_, cells)) = module.declared() {
                    cells.holds.restore(write.control, write.hold, write.prev);
                    if let Some(slot) = cells.staged.slots.get(usize::from(write.control.0)) {
                        slot.hold.store(0, Relaxed);
                    }
                }
                Outcome::Refused(refusal)
            }
        };
        outcomes.flushed(write.id, outcome, write.timed);
    }
    if let Some((_, cells)) = module.declared() {
        cells.staged.len.store(kept, Relaxed);
    }
}

#[cfg(test)]
mod tests;
