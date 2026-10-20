//! Structural edits timed as control requests are: a publication that
//! lands on the sample its `when` resolves to, rather than at the next
//! block start.
//!
//! A timed edit is prepared and validated on a control thread against the
//! publisher's mirror of generation `g`, and queued as an edit request
//! with a `when` (see `publish::timed`). Until it lands the publisher stays
//! at `g`: every request and input write submitted meanwhile is resolved
//! against the graph that is playing, and applies to it.
//!
//! On the audio thread the request waits where a control request with the
//! same `when` waits (the pending store, the beat watches or the
//! conditions), its target the module index [`TIMED_EDIT_MODULE`], which
//! no install maps: any other edit installing first refuses it
//! (`TargetGone`). When due, [`SignalGraph::take_timed_edit`] takes its
//! publication, which [`SignalGraph::land_timed_edit`] installs at that
//! segment start, after every request due at the same sample has applied
//! to the graph it replaces, as an immediate edit does at a block start.
//! Segments run as blocks of their own, so an install between two is the
//! install a block start makes, carries and all.
//!
//! # Staleness: refused, never re-validated
//!
//! A timed edit applies only to the graph it was prepared for. Whichever
//! comes first wins, decided once by its [`Fate`]:
//!
//! - the audio thread landing it (waiting → landed), or
//! - the publisher publishing any other change while it waits (waiting →
//!   cancelled). That change was prepared against the graph that is
//!   playing, so it stands, and the timed edit is refused `TargetGone`
//!   when due, or as soon as that change installs.
//!
//! Re-validating instead would mean replaying the edit onto a graph it
//! never saw: a timed reload would undo whatever changed in between.
//!
//! A request or input write submitted after the edit lands but before the
//! publisher adopts it is still tagged `g`. For now it finds its target
//! gone; a bridge mapping it to the graph the edit installed comes next.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use super::{AudioLink, Publication, SignalGraph};
use crate::control_request::{Outcome, Refusal};
use crate::payload::{Payload, Retirer};

/// The module index a timed edit's request targets: none, so no install
/// maps it, and a condition watching for it applies it only at a segment
/// start.
///
/// Nothing may index a module by it: `apply_ranked` takes the edit before
/// any lookup, and every other audio-side path reaches modules through
/// `get_index`/`get_index_mut`, which find none. A table keyed by target
/// (holds, ramps) must skip it.
// Queued by `publish::timed` (the next change).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const TIMED_EDIT_MODULE: usize = usize::MAX;

const WAITING: u8 = 0;
const LANDED: u8 = 1;
#[cfg_attr(not(test), allow(dead_code))]
const CANCELLED: u8 = 2;
const DROPPED: u8 = 3;

/// Where a timed edit stands, shared by the publisher, which may cancel
/// it, and its publication, which the audio thread lands. It moves from
/// waiting exactly once.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fate(Arc<AtomicU8>);

// Decided by `publish::timed` (the next change).
#[cfg_attr(not(test), allow(dead_code))]
impl Fate {
    fn settle(&self, to: u8) -> bool {
        self.0
            .compare_exchange(WAITING, to, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Audio thread: lands the edit, unless it was cancelled.
    fn land(&self) -> bool {
        self.settle(LANDED)
    }

    /// Publisher: cancels the edit, unless it landed (or was dropped).
    pub(crate) fn cancel(&self) -> bool {
        self.settle(CANCELLED)
    }

    /// Whether it landed.
    pub(crate) fn landed(&self) -> bool {
        self.0.load(Ordering::Acquire) == LANDED
    }

    /// The guard its publication carries.
    pub(crate) fn guard(&self) -> FateGuard {
        FateGuard(self.clone())
    }
}

/// Carried by a timed edit's publication: freed while the edit still waits
/// (refused or expired before it was due), it marks the edit dropped, so
/// the publisher stops waiting for it. Freed off the audio thread.
#[derive(Debug)]
pub(crate) struct FateGuard(Fate);

impl Drop for FateGuard {
    fn drop(&mut self) {
        self.0.settle(DROPPED);
    }
}

impl SignalGraph {
    /// Takes the publication a timed edit carries, now due, to install at
    /// this segment start: lands it, or refuses it `TargetGone` when it was
    /// cancelled, or was prepared for another generation than the one
    /// installed. Waits, refused, while a retirement is held, so the link
    /// never holds two. Never drops it: a refused one is retired.
    /// Allocation-, free- and lock-free.
    pub(in crate::invention::graph) fn take_timed_edit(
        &mut self,
        payload: Payload,
        retirer: &mut Retirer,
    ) -> Result<(), Outcome> {
        let publication = match payload.take_owned::<Publication>() {
            Ok(publication) => publication,
            Err(other) => {
                retirer.retire(other);
                return Err(Outcome::Refused(Refusal::Unsupported));
            }
        };
        let fits = |link: &mut AudioLink| {
            link.landed.is_none()
                && publication.generation == link.installed + 1
                && link.flush_held()
        };
        let lands = self.link.as_mut().is_some_and(|link| {
            fits(link) && publication.fate.as_ref().is_some_and(|f| f.0.land())
        });
        match self.link.as_mut() {
            Some(link) if lands => {
                link.landed = Some(publication);
                Ok(())
            }
            _ => {
                retirer.retire(publication);
                Err(Outcome::Refused(Refusal::TargetGone))
            }
        }
    }

    /// Installs the timed edit taken this segment start, if any, as
    /// `drain_link` installs an edit. Returns whether it installed one.
    /// Allocation-, free- and lock-free, but for the recompile an install
    /// may fall back to (as at a block start).
    pub(in crate::invention::graph) fn land_timed_edit(&mut self) -> bool {
        let Some(mut link) = self.link.take() else {
            return false;
        };
        let Some(mut publication) = link.landed.take() else {
            self.link = Some(link);
            return false;
        };
        let previous = link.installed;
        self.install(&mut publication);
        link.installed = publication.generation;
        link.applied.fetch_add(1, Ordering::Relaxed);
        self.remap_requests(link.installed, Some((previous, &publication)));
        if self.topo_dirty {
            self.recompile();
        }
        link.retire(publication);
        self.link = Some(link);
        true
    }
}
