//! Who wins a control when writes conflict (A4): tiers, priority, and the
//! hold a higher tier keeps against lower ones.
//!
//! # Rules
//!
//! - A write ranks by its source's tier, user = agent > script >
//!   automation, then by its `priority` within the tier. At one sample the
//!   higher rank wins whatever the order, and at equal rank the later
//!   write. A write is weighed at the sample it was due at, so one
//!   applied late loses to a higher rank's write at or after its due
//!   sample, as it would have on time. (Only that way: a late write is not
//!   undone by a lower rank's applied before it arrived, nor weighed
//!   against a hold since released.) A write losing to a higher tier is
//!   [`Outcome::Overridden`], one losing within its tier
//!   [`Outcome::Superseded`].
//! - A write that applies holds its control against lower-ranked writes
//!   due at or before its due sample. A lower-tier request due after it is
//!   that tier's next scheduled event on the control (a repeating grid's
//!   every beat included), and applies, taking the control back.
//! - A write above the automation tier holds its control against
//!   automation until automation's next scheduled event on it, a
//!   scheduler's entry: between entries its ramp writes are dropped
//!   ([`Holds::holds_off_automation`]), so a running ramp is abandoned,
//!   and its next entry resumes ([`Holds::release`]), ramping on from
//!   where the hold left the control. An entry firing at the sample the
//!   write was due at loses to it, and the control stays held until the
//!   entry after it; whatever the block size, since an entry's sample is
//!   the processing frame it fires on ([`FrameScope`]). A development ramp
//!   runs on while any alias takes its writes; a held alias drops them
//!   until automation's next event on it, and an inner entry frees the
//!   alias it writes, not the development's own word (which only weighs
//!   requests due at or before it).
//! - Through a development, the hold covers every inner control its key
//!   reaches: a write through another key reaching one of them is weighed
//!   against it, and automation writing them is held off. Each is released
//!   by automation's next event on it, as any control is. One held alias
//!   holds off the whole write (it loses to the strongest hold it meets),
//!   and two keys sharing an inner control are two targets: at one sample
//!   the one applied first holds the other off, so they resolve in the
//!   order they apply, not by rank.
//! - A write that rewrites other controls of its module, as a melody's
//!   whole-scale replacement rewrites every position, is weighed and holds
//!   as a development's key does: against every control it rewrites
//!   ([`crate::Module::for_each_alias`] names them). A control that only
//!   decides which others are in play, as the melody's count does,
//!   rewrites none, so moving it (a ramp included) never overwrites a held
//!   one: the hold wins. Automation is weighed only against the hold on the
//!   control it writes, so a rewriting control is a payload, which no
//!   automation writes (every one is today).
//! - Events (triggers) are never held: two are two events. A key that
//!   reaches one is an event itself, and a rewrite never names one, so a
//!   write that holds never reaches an event.
//! - A payload control ranks and holds as a value does. No automation
//!   writes one, so its hold only weighs requests. A payload that loses is
//!   retired with the request, never dropped on the audio thread.
//!
//! # State
//!
//! One word per declared control, in its module's
//! [`ControlCells`](super::ControlCells), allocated as the module is built:
//! so the state is fixed-size, capped at the table's length, and a write to
//! an index past it is refused before it gets here. It belongs to the
//! instance: a module that survives an edit or a reload keeps its holds,
//! and one rebuilt starts with none. Only the thread running the module
//! reads or writes it (the audio thread, or an offline render under its
//! graph's lock), so `Relaxed` is enough, as for automation slots.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use super::cells::ControlCells;
use super::declare::ControlTable;
use super::pending::Outcome;
use super::request::{ControlIndex, Request, Source};
use crate::Module;

/// A write's standing on a control: its tier, then its priority within
/// the tier. Higher wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Rank {
    tier: u8,
    priority: i16,
}

/// The tier of automation, which holds nothing against anyone.
const AUTOMATION: u8 = 0;

impl Rank {
    /// A user's write at priority 0: an offline render's control write.
    pub(crate) const USER: Self = Self::new(Source::User, 0);

    pub(crate) const fn new(source: Source, priority: i16) -> Self {
        let tier = match source {
            Source::User | Source::Agent => 2,
            Source::Script => 1,
            Source::Automation => AUTOMATION,
        };
        Self { tier, priority }
    }

    /// The rank `request` writes at, or `None` for an event, which is
    /// never held and never holds.
    pub(crate) fn of(request: &Request) -> Option<Self> {
        (!request.event).then(|| Self::new(request.source, request.priority))
    }

    /// What a write of this rank reports on losing to one of `winner`'s.
    pub(crate) fn loses_to(self, winner: Self) -> Outcome {
        if self.tier < winner.tier {
            Outcome::Overridden
        } else {
            Outcome::Superseded
        }
    }
}

thread_local! {
    /// The sample frame 0 of the modules processing on this thread falls
    /// on, or `NOWHERE` outside processing (see [`Holds::release`]).
    static FRAME_ZERO: Cell<u64> = const { Cell::new(NOWHERE) };
}

/// Outside processing.
const NOWHERE: u64 = u64::MAX;

/// The sample frame 0 of the modules processing on this thread falls on,
/// or `None` outside processing. Allocation-, free- and lock-free.
#[inline]
pub(crate) fn frame_zero() -> Option<u64> {
    let zero = FRAME_ZERO.with(Cell::get);
    (zero != NOWHERE).then_some(zero)
}

/// Marks frame 0 of the modules about to process on this thread as sample
/// `sample` until dropped, restoring the previous mark, so scopes nest: a
/// graph enters one per segment, and per sample where it processes a
/// feedback group (or a development its inner graph) a sample at a time.
/// Allocation-, free- and lock-free.
#[must_use = "the mark ends when this value is dropped"]
pub(crate) struct FrameScope {
    outer: u64,
}

impl FrameScope {
    #[inline]
    pub(crate) fn enter(sample: u64) -> Self {
        Self {
            outer: FRAME_ZERO.with(|zero| zero.replace(sample)),
        }
    }

    /// Frame `offset` of the enclosing scope as the new frame 0 (none
    /// outside processing).
    #[inline]
    pub(crate) fn frame(offset: usize) -> Self {
        let zero = FRAME_ZERO.with(Cell::get);
        Self::enter(match zero {
            NOWHERE => NOWHERE,
            zero => zero + offset as u64,
        })
    }
}

impl Drop for FrameScope {
    #[inline]
    fn drop(&mut self) {
        FRAME_ZERO.with(|zero| zero.set(self.outer));
    }
}

/// What a write is weighed by: its rank, and the sample it was due at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Standing {
    pub(crate) rank: Rank,
    pub(crate) due: u64,
}

impl Standing {
    /// An offline render's control write: a user's, due `now`.
    pub(crate) const fn user(now: u64) -> Self {
        Self {
            rank: Rank::USER,
            due: now,
        }
    }

    /// The standing of `request` due at `due`, or `None` for an event.
    pub(crate) fn of(request: &Request, due: u64) -> Option<Self> {
        Rank::of(request).map(|rank| Self { rank, due })
    }
}

/// No hold.
const FREE: u64 = 0;
/// The sample a hold's write was due at, kept to 45 bits (about 5.8
/// years at 192 kHz), past which comparisons wrap.
const SAMPLE_BITS: u32 = 45;
const SAMPLE_MASK: u64 = (1 << SAMPLE_BITS) - 1;

fn encode(rank: Rank, now: u64) -> u64 {
    (now & SAMPLE_MASK) << 19
        | u64::from(rank.tier) << 17
        | u64::from(rank.priority as u16) << 1
        | 1
}

/// The rank and sample of a hold.
fn decode(word: u64) -> Option<(Rank, u64)> {
    let tier = (word >> 17 & 0b11) as u8;
    let priority = (word >> 1) as u16 as i16;
    (word != FREE).then_some((Rank { tier, priority }, word >> 19))
}

/// The hold on each of a module's declared controls (see the module docs).
pub(crate) struct Holds(Box<[AtomicU64]>);

impl Holds {
    /// No holds, for `len` controls. Allocates: control thread.
    pub(super) fn new(len: usize) -> Self {
        Self((0..len).map(|_| AtomicU64::new(FREE)).collect())
    }

    fn word(&self, index: ControlIndex) -> u64 {
        self.0
            .get(usize::from(index.0))
            .map_or(FREE, |word| word.load(Ordering::Relaxed))
    }

    fn set(&self, index: ControlIndex, word: u64) {
        if let Some(hold) = self.0.get(usize::from(index.0)) {
            hold.store(word, Ordering::Relaxed);
        }
    }

    /// Whether a write of `standing` may apply to `index`, or what it
    /// reports: it loses only to a higher-ranked write due at or after its
    /// own due sample (the same sample, when both are on time). A write
    /// due later is its tier's next event.
    pub(crate) fn admits(&self, index: ControlIndex, standing: Standing) -> Result<(), Outcome> {
        let Standing { rank, due } = standing;
        match decode(self.word(index)) {
            Some((held, at)) if held > rank && due & SAMPLE_MASK <= at => Err(rank.loses_to(held)),
            _ => Ok(()),
        }
    }

    /// Records that a write of `standing` applied to `index`.
    pub(crate) fn take(&self, index: ControlIndex, standing: Standing) {
        self.set(index, encode(standing.rank, standing.due));
    }

    /// Whether the latest write to take `index` was one of `standing`: no
    /// write since took it, and automation's next event has not freed it.
    pub(crate) fn kept(&self, index: ControlIndex, standing: Standing) -> bool {
        self.word(index) == encode(standing.rank, standing.due)
    }

    /// Whether a tier above automation holds `index`: a ramp's writes to
    /// it are dropped until automation's next event on it.
    pub(crate) fn holds_off_automation(&self, index: ControlIndex) -> bool {
        decode(self.word(index)).is_some_and(|(held, _)| held.tier > AUTOMATION)
    }

    /// Automation's next scheduled event on `index`, firing on frame
    /// `frame` of the processing modules: it takes the control back,
    /// unless the hold's write was due at that sample or after it.
    /// Outside processing, it always does.
    pub(crate) fn release(&self, index: ControlIndex, frame: usize) {
        let zero = FRAME_ZERO.with(Cell::get);
        let outranks = |at| zero != NOWHERE && (zero + frame as u64) & SAMPLE_MASK <= at;
        match decode(self.word(index)) {
            Some((held, at)) if held.tier > AUTOMATION && outranks(at) => {}
            _ => self.set(index, FREE),
        }
    }
}

/// Whether a write of `standing` may apply to declared `control` of
/// `module` ([`Holds::admits`]): weighed against the hold on it, and on
/// every control it reaches (a development key's inner controls, or the
/// controls it rewrites), which another write may hold. The strongest
/// loss decides, whatever the order they are met in. Allocation-, free-
/// and lock-free.
pub(crate) fn admits_hold<M: Module + ?Sized>(
    module: &M,
    control: ControlIndex,
    standing: Standing,
) -> Result<(), Outcome> {
    let mut admitted = Ok(());
    let mut weigh = |_: &ControlTable, cells: &ControlCells, index| {
        // A loss to a higher tier outweighs one within the tier.
        if admitted != Err(Outcome::Overridden) {
            if let Err(lost) = cells.holds.admits(index, standing) {
                admitted = Err(lost);
            }
        }
    };
    if let Some((table, cells)) = module.declared() {
        weigh(table, cells, control);
    }
    module.for_each_alias(control, &mut weigh);
    admitted
}

/// Records that a write of `standing` applied to declared `control` of
/// `module` ([`Holds::take`]), and to every control it reaches (a
/// development key's inner controls, or the controls it rewrites), so
/// automation writing those directly is held off too. An event control is
/// never held. Allocation-, free- and lock-free.
pub(crate) fn take_hold<M: Module + ?Sized>(module: &M, control: ControlIndex, standing: Standing) {
    let Some((table, cells)) = module.declared() else {
        return;
    };
    if table.decl(control).is_some_and(|(decl, _)| decl.event) {
        return;
    }
    cells.holds.take(control, standing);
    module.for_each_alias(control, &mut |table, cells, index| {
        // A table scan, so checked only in debug builds: the hold path
        // stays independent of the table's size.
        debug_assert!(
            !table.decl(index).is_some_and(|(decl, _)| decl.event),
            "a write that holds reached an event control"
        );
        cells.holds.take(index, standing);
    });
}

/// Whether every hold a write of `standing` to declared `control` of
/// `module` took ([`take_hold`]) is still its own ([`Holds::kept`]): on
/// every control it reaches too. Allocation-, free- and lock-free.
pub(crate) fn kept_hold<M: Module + ?Sized>(
    module: &M,
    control: ControlIndex,
    standing: Standing,
) -> bool {
    let Some((_, cells)) = module.declared() else {
        return false;
    };
    let mut kept = cells.holds.kept(control, standing);
    module.for_each_alias(control, &mut |_, cells, index| {
        kept &= cells.holds.kept(index, standing);
    });
    kept
}

#[cfg(test)]
mod derived_tests;
#[cfg(test)]
mod tests;
