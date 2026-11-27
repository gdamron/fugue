//! A timed edit landing on an offline render's graph, which no audio link
//! reaches: the render holds what lands itself, as a link does, and
//! installs it at the same segment start a live graph would (see
//! `landing`), after every request due there has applied to the graph it
//! replaces. What the install displaces stays with the graph, freed on the
//! control thread as the render next settles (see `offline_edits`), so a
//! render that lands an edit neither allocates nor frees, but for the
//! recompile an install may fall back to, as at a live block start.

use super::{Publication, SignalGraph};
use crate::control_request::{Outcome, Refusal};
use crate::payload::Retirer;

impl SignalGraph {
    /// [`Self::take_timed_edit`] on an offline render: takes `publication`
    /// to install at this segment start, unless its edit was cancelled, it
    /// was prepared for another graph than the one playing (an edit has
    /// installed since), or another waits to install. One refused is
    /// retired. Allocation-, free- and lock-free.
    pub(super) fn take_offline_edit(
        &mut self,
        publication: Box<Publication>,
        retirer: &mut Retirer,
    ) -> Result<(), Outcome> {
        let generation = self.offline_edits + 1;
        let slot = (self.offline.as_mut())
            .map(|offline| &mut offline.edits.landing)
            .filter(|slot| slot.is_none() && publication.generation == generation);
        match slot {
            Some(slot) if publication.fate.as_ref().is_some_and(|fate| fate.land()) => {
                *slot = Some(publication);
                Ok(())
            }
            _ => {
                retirer.retire(publication);
                Err(Outcome::Refused(Refusal::TargetGone))
            }
        }
    }

    /// [`Self::land_timed_edit`] on an offline render: installs the timed
    /// edit taken this segment start, if any, as [`Self::install_offline`]
    /// installs an edit, and its graph's `ended` gates with it. Keeps what
    /// it displaced for the control thread to free. Returns whether it
    /// installed one. Allocation-, free- and lock-free, but for the
    /// recompile an install may fall back to.
    pub(super) fn land_offline_edit(&mut self) -> bool {
        let Some(publication) =
            (self.offline.as_mut()).and_then(|offline| offline.edits.landing.take())
        else {
            return false;
        };
        let installed = self.request_generation();
        let land = |graph: &mut Self, publication: &mut Box<Publication>| {
            graph.install(publication);
            graph.remap_requests(installed + 1, Some((installed, &**publication)));
            // No publisher keeps an ordering here (see `install_offline`).
            graph.published = false;
            graph.offline_edits += 1;
            if graph.topo_dirty {
                graph.recompile();
            }
            let gates = graph.offline.as_mut().and_then(|o| o.edits.gates.as_mut());
            if let (Some(watch), Some(gates)) = (graph.end_watch.as_mut(), gates) {
                watch.take_gates(gates);
            }
        };
        // Kept, displaced, through a panic too (see `unwind`).
        let keep = |graph: &mut Self, publication| match graph.offline.as_mut() {
            Some(offline) if offline.edits.displaced.is_none() => {
                offline.edits.displaced = Some(publication);
            }
            _ => graph.keep(publication),
        };
        self.restoring(publication, land, keep);
        true
    }

    /// The timed edit an offline render took this segment start, until it
    /// installs.
    pub(in crate::invention::graph) fn offline_landing(&self) -> Option<&Publication> {
        self.offline.as_ref()?.edits.landing.as_deref()
    }
}
