//! Payloads a publication hands to surviving modules as it installs.
//!
//! A surviving `control_scheduler` writes its targets through their
//! surfaces and cells, so when a change rebuilds a target the scheduler
//! must take a schedule re-resolved against the new instance in the very
//! block that installs it, or a scheduled value lands on the instance the
//! change replaced. Queued as a request behind the edit, it could miss
//! that block (the audio thread may drain between the two) or find the
//! queue full. So the commit packs it into the publication itself, and
//! [`SignalGraph::install`] hands it over before any module runs. A
//! schedule for a module the publication keeps as it is (a live write
//! recompiling the process order) goes with the requests due as the edit
//! installs instead, before the beats then play (see
//! [`Handover::with_requests`]).

use super::{Publication, SignalGraph};
use crate::control_request::ControlIndex;
use crate::payload::{Payload, Retired};

/// A payload for a surviving module's declared payload control, applied
/// as its publication installs.
pub(crate) struct Handover {
    /// The module's index in the publication's (final) order.
    pub(crate) module_idx: usize,
    pub(crate) control: ControlIndex,
    /// The payload until installed; one the module refused stays here.
    pub(crate) payload: Option<Payload>,
    /// What the payload replaced. Both leave the audio thread with the
    /// retired publication, to be freed by the reclaimer.
    pub(crate) replaced: Option<Retired>,
    /// Applied with the requests due as the edit installs, before beats
    /// play (see [`Self::with_requests`]), rather than at the install.
    pub(crate) with_requests: bool,
}

impl Handover {
    /// A handover of `payload` to `control` of the module at `module_idx`.
    pub(crate) fn new(module_idx: usize, control: ControlIndex, payload: Payload) -> Self {
        Self {
            module_idx,
            control,
            payload: Some(payload),
            replaced: None,
            with_requests: false,
        }
    }

    /// A handover of `payload` to a module the publication keeps as it is
    /// running, applied as a request submitted just before the edit would
    /// be: after the requests due at the install's sample, before any beat
    /// then plays, as the edit's publication is taken. So the schedule it
    /// replaces plays nothing at that sample, and a schedule handed over
    /// before its first beat plays its step 0 there.
    pub(crate) fn with_requests(
        module_idx: usize,
        control: ControlIndex,
        payload: Payload,
    ) -> Self {
        Self {
            with_requests: true,
            ..Self::new(module_idx, control, payload)
        }
    }
}

impl Publication {
    /// Whether it hands the running module at `running` (in the order it
    /// replaces) a schedule at the install, re-resolved against modules it
    /// rebuilds: that module's table plays the sample's entries after it.
    /// Allocation-free.
    pub(in crate::invention::graph) fn rebinds(&self, running: usize) -> bool {
        self.remap.get(running).is_some_and(|module_idx| {
            self.handovers
                .iter()
                .any(|handover| !handover.with_requests && handover.module_idx == module_idx)
        })
    }
}

impl SignalGraph {
    /// Applies each of `publication`'s handovers made
    /// [`Handover::with_requests`] to the running module it names, which
    /// survives it: the graph is the one the publication replaces, and the
    /// id finds the instance [`Self::install`] moves into its slot. One
    /// naming anything else is left for [`Self::hand_over`].
    /// Allocation-, free- and lock-free.
    pub(in crate::invention::graph) fn hand_over_with_requests(
        &mut self,
        publication: &mut Publication,
    ) {
        for handover in &mut publication.handovers {
            if !handover.with_requests
                || publication.survivor.get(handover.module_idx) != Some(&true)
            {
                continue;
            }
            let id = publication.modules.get_index(handover.module_idx);
            let running = id.and_then(|(id, _)| self.modules.get_mut(id.as_str()));
            let (Some(instance), Some(payload)) = (running, handover.payload.take()) else {
                continue;
            };
            match instance
                .module_mut()
                .apply_payload(handover.control, payload)
            {
                Ok(replaced) => handover.replaced = Some(replaced),
                Err((_, payload)) => handover.payload = Some(payload),
            }
        }
    }

    /// Applies each of `publication`'s handovers to the survivor it names,
    /// once installed. A handover naming anything but a survivor is left
    /// as it is. Allocation-, free- and lock-free.
    pub(super) fn hand_over(&mut self, publication: &mut Publication) {
        for handover in &mut publication.handovers {
            if publication.survivor.get(handover.module_idx) != Some(&true) {
                continue;
            }
            let Some((_, instance)) = self.modules.get_index_mut(handover.module_idx) else {
                continue;
            };
            // Never dropped here: one refused goes back in its place.
            let Some(payload) = handover.payload.take() else {
                continue;
            };
            match instance
                .module_mut()
                .apply_payload(handover.control, payload)
            {
                Ok(replaced) => handover.replaced = Some(replaced),
                Err((_, payload)) => handover.payload = Some(payload),
            }
        }
    }
}
