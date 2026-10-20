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
//! # The bridge
//!
//! Requests and input writes submitted after the edit lands but before the
//! publisher adopts it are still tagged `g`. So the publication the landing
//! retires is kept as the link's *bridge*, and what arrives tagged `g` is
//! mapped through its survivor remap, as writes are through any install.
//! Once the publisher has adopted the edit (`published` reaches its
//! generation) and nothing tagged `g` is left in either queue, the bridge
//! goes back to the control thread as any retired publication does.

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
#[cfg_attr(not(test), allow(dead_code))] // until the runtime calls `commit_when`
pub(crate) const TIMED_EDIT_MODULE: usize = usize::MAX;

const WAITING: u8 = 0;
const LANDED: u8 = 1;
const CANCELLED: u8 = 2;
const DROPPED: u8 = 3;

/// Where a timed edit stands, shared by the publisher, which may cancel
/// it, and its publication, which the audio thread lands. It moves from
/// waiting exactly once.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fate(Arc<AtomicU8>);

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

    /// Where it stands, read once: still waiting (`None`), landed
    /// (`Some(true)`), or cancelled or dropped (`Some(false)`).
    pub(crate) fn settled(&self) -> Option<bool> {
        match self.0.load(Ordering::Acquire) {
            WAITING => None,
            state => Some(state == LANDED),
        }
    }

    /// The guard its publication carries.
    #[cfg_attr(not(test), allow(dead_code))] // until the runtime calls `commit_when`
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

impl AudioLink {
    /// The generation a bridge maps from, with the bridge.
    pub(super) fn bridge(&self) -> Option<(u64, &Publication)> {
        let bridge = self.bridge.as_deref()?;
        Some((bridge.generation - 1, bridge))
    }
}

impl SignalGraph {
    /// Takes the publication a timed edit carries, now due, to install at
    /// this segment start: lands it, or refuses it `TargetGone` when it was
    /// cancelled, or was prepared for another generation than the one
    /// installed. Refused too, defensively, while an earlier landing's
    /// bridge still maps. Never drops it: a refused one is retired.
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
        let fits = |link: &AudioLink| {
            link.landed.is_none()
                && link.bridge.is_none()
                && publication.generation == link.installed + 1
        };
        let lands = match self.link.as_mut() {
            Some(link) if fits(link) => publication.fate.as_ref().is_some_and(|f| f.0.land()),
            _ => false,
        };
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
    /// `drain_link` installs an edit, keeping the publication it retires as
    /// the link's bridge. Returns whether it installed one.
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
        link.bridge = Some(publication);
        self.link = Some(link);
        true
    }

    /// Hands the bridge back once nothing can need it: the publisher has
    /// adopted its edit (`adopted`, read before the queues were drained)
    /// and neither the request queue nor the input ring holds anything
    /// tagged with the generation it maps from. Drains the input ring
    /// through it to find out. Waits while a retirement is held, so the
    /// link never holds two. Returns whether it went.
    pub(super) fn release_bridge(&mut self, link: &mut AudioLink, adopted: bool) -> bool {
        let Some(bridge) = link.bridge.take() else {
            return false;
        };
        let previous = bridge.generation - 1;
        let drained = adopted
            && self
                .requests
                .as_mut()
                .is_none_or(|drain| drain.drained_through(previous))
            && self.drain_inputs(link, Some((previous, &bridge)))
            // Never two retirements held at once.
            && link.flush_held();
        if !drained {
            link.bridge = Some(bridge);
            return false;
        }
        link.retire(bridge);
        true
    }
}
