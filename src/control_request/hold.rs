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
//!   that tier's next scheduled event on the control, and applies, taking
//!   the control back.
//! - Events (triggers) are never held: two are two events.
//! - Requests timed in beats are not ranked yet.
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

use std::sync::atomic::{AtomicU64, Ordering};

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
}

/// Whether a write of `standing` may apply to declared `control` of
/// `module` ([`Holds::admits`]). Allocation-, free- and lock-free.
pub(crate) fn admits_hold<M: Module + ?Sized>(
    module: &M,
    control: ControlIndex,
    standing: Standing,
) -> Result<(), Outcome> {
    match module.declared() {
        Some((_, cells)) => cells.holds.admits(control, standing),
        None => Ok(()),
    }
}

/// Records that a write of `standing` applied to declared `control` of
/// `module` ([`Holds::take`]). An event control is never held.
/// Allocation-, free- and lock-free.
pub(crate) fn take_hold<M: Module + ?Sized>(module: &M, control: ControlIndex, standing: Standing) {
    let Some((table, cells)) = module.declared() else {
        return;
    };
    if table.decl(control).is_some_and(|(decl, _)| decl.event) {
        return;
    }
    cells.holds.take(control, standing);
}

#[cfg(test)]
mod tests;
