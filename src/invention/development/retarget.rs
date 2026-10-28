//! A payload written through a development carries the inner order it
//! leaves, so a `schedule` that retargets an inner scheduler plays in the
//! order a development loaded with it would run, live and offline alike.
//!
//! A development orders its inner modules by their routes and by what each
//! writes (see [`process_order`]). Writing an inner scheduler's `schedule`
//! can change what it writes, and with it that order. The development's
//! surface records the new schedule first, computes the order on the
//! control thread, and wraps the payload with it ([`Reordered`]). The
//! development unwraps it on the thread that applies payloads (the audio
//! thread, live) and, in the same call, hands the inner payload to its
//! module and copies the order over its own: the new schedule never plays
//! in the old order, nor the old one in the new. Copying in place
//! allocates nothing: the order always lists every inner module once.

use super::compiled_graph::process_order;
use super::*;
use crate::control_request::Refusal;
use crate::payload::{Payload, RetireOnUnwind, Retired};

/// What a development's inner order is computed from, by inner module
/// index: the ids, and the routes as `(from, to)` pairs in routing order,
/// as the development compiled them.
pub(super) struct InnerOrder {
    pub(super) ids: Vec<String>,
    pub(super) routes: Vec<(usize, usize)>,
}

impl InnerOrder {
    /// The order for what each inner module writes now, as `surfaces`
    /// report it (see [`ControlSurface::control_targets`]). Control thread:
    /// allocates.
    pub(super) fn compute(
        &self,
        surfaces: &IndexMap<String, ControlSurfaceInstance>,
    ) -> Box<[usize]> {
        let indexes: HashMap<&str, usize> = self
            .ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let targets: Vec<Vec<usize>> = self
            .ids
            .iter()
            .map(|id| {
                let written = surfaces
                    .get(id)
                    .map(|s| s.control_targets())
                    .unwrap_or_default();
                written
                    .iter()
                    .filter_map(|target| indexes.get(target.as_str()).copied())
                    .collect()
            })
            .collect();
        process_order(self.ids.len(), &self.routes, &targets)
            .0
            .into_boxed_slice()
    }
}

/// A payload for an inner module and the inner order it leaves.
struct Reordered {
    inner: Option<Payload>,
    order: Box<[usize]>,
    /// What the inner payload replaced, retired with this.
    replaced: Option<Retired>,
}

/// Wraps `inner` with `order`. Control thread: allocates.
pub(super) fn reordered(inner: Payload, order: Box<[usize]>) -> Payload {
    Payload::owned(Box::new(Reordered {
        inner: Some(inner),
        order,
        replaced: None,
    }))
}

/// Applies `payload` with `apply`, and the order it carries, if any, over
/// `current` once `apply` takes the inner payload. Returns what it
/// replaced, or the payload back, refused, as it came. Allocation- and
/// free-free: the audio thread applies payloads.
pub(super) fn apply_reordered(
    current: &mut [usize],
    payload: Payload,
    apply: impl FnOnce(Payload) -> Result<Retired, (Refusal, Payload)>,
) -> Result<Retired, (Refusal, Payload)> {
    let reordered = match payload.take_owned::<Reordered>() {
        Ok(reordered) => reordered,
        // Built by no development's surface (a handover): no order.
        Err(payload) => return apply(payload),
    };
    let mut reordered = RetireOnUnwind::new(reordered);
    let Some(inner) = reordered.inner.take() else {
        return Err((Refusal::Invalid, Payload::owned(reordered.into_inner())));
    };
    // An inner module panicking leaves the box for the graph to keep, not
    // to free here.
    let applied = apply(inner);
    let mut reordered = reordered.into_inner();
    match applied {
        Ok(replaced) => {
            if reordered.order.len() == current.len() {
                current.copy_from_slice(&reordered.order);
            }
            reordered.replaced = Some(replaced);
            Ok(Retired::from(reordered))
        }
        Err((refusal, inner)) => {
            reordered.inner = Some(inner);
            Err((refusal, Payload::owned(reordered)))
        }
    }
}
