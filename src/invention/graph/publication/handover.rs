//! Payloads a publication hands to surviving modules as it installs.
//!
//! A surviving `control_scheduler` writes its targets through their
//! surfaces and cells, so when a change rebuilds a target the scheduler
//! must take a schedule re-resolved against the new instance in the very
//! block that installs it, or a scheduled value lands on the instance the
//! change replaced. Queued as a request behind the edit, it could miss
//! that block (the audio thread may drain between the two) or find the
//! queue full. So the commit packs it into the publication itself, and
//! [`SignalGraph::install`] hands it over before any module runs.

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
}

impl Handover {
    /// A handover of `payload` to `control` of the module at `module_idx`.
    pub(crate) fn new(module_idx: usize, control: ControlIndex, payload: Payload) -> Self {
        Self {
            module_idx,
            control,
            payload: Some(payload),
            replaced: None,
        }
    }
}

impl SignalGraph {
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
