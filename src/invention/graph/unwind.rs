//! Keeping what a render holds across a module's panic.
//!
//! The audio thread takes the request drain and the audio link out of the
//! graph while it works on them beside it, and holds an edit's publication
//! between popping it and handing it back. A module panicking meanwhile (in
//! `apply`, `set_input`, `process`, a handover) would unwind through those
//! locals and free them on the rendering thread: the pending store and the
//! payloads in it, the publication and the modules it holds. With the drain
//! the queue's consumer would go too, without emptying the queue (nothing
//! may be freed while unwinding): later submissions would queue with
//! nothing to read them, and an edit left in it, holding modules whose
//! controls submit to the queue, would keep the queue and itself alive for
//! good.
//!
//! So every spot that takes something out runs under
//! [`SignalGraph::restoring`]: should it panic, what it took goes back into
//! the graph before the unwind goes on, a publication in hand into
//! [`SignalGraph::unwound`]. As the panic leaves the block,
//! `process_block` fails the graph ([`SignalGraph::fail`]): its request
//! queue closes, so the publisher refuses every later submission at once
//! (`AudioThreadStopped`), and every later block renders silence and
//! touches nothing. The graph keeps it all for its teardown, on a control
//! thread once the backend stops (a device stream renders silence after a
//! render panic; a null backend settles no more), where the drain empties
//! the queue as it goes.
//!
//! A payload moved into the panicking module itself (`apply_payload`, a
//! handover) is beyond the graph's reach: the unwind drops it inside the
//! module, so it is leaked there instead (`crate::payload::RenderScope`).
//!
//! Catching costs nothing until something panics. The unwind then goes on
//! through [`resume_unwind`], which runs no panic hook again.

use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use super::publication::{AudioLink, Publication};
use super::requests::RequestDrain;
use super::SignalGraph;

impl SignalGraph {
    /// Runs `body` on the graph and `taken`, something taken out of the
    /// graph to work on beside it, then hands `taken` to `restore`: also
    /// when `body` panics, before the unwind goes on, so the unwind never
    /// frees it. Allocation-, free- and lock-free until a panic.
    pub(super) fn restoring<T, R>(
        &mut self,
        mut taken: T,
        body: impl FnOnce(&mut Self, &mut T) -> R,
        restore: impl FnOnce(&mut Self, T),
    ) -> R {
        let result = catch_unwind(AssertUnwindSafe(|| body(self, &mut taken)));
        restore(self, taken);
        result.unwrap_or_else(|panic| resume_unwind(panic))
    }

    /// Runs `body` with the request drain taken out of the graph, and puts
    /// it back, as [`Self::restoring`] does. `None`, running nothing, for a
    /// graph without one.
    pub(super) fn with_drain<R>(
        &mut self,
        body: impl FnOnce(&mut Self, &mut RequestDrain) -> R,
    ) -> Option<R> {
        let drain = self.requests.take()?;
        let restore = |graph: &mut Self, drain| graph.requests = Some(drain);
        Some(self.restoring(drain, body, restore))
    }

    /// Runs `body` with the audio link taken out of the graph, and puts it
    /// back, as [`Self::restoring`] does. `body` holds a popped edit's
    /// publication in the slot it is given until it hands it on: one still
    /// there is kept in [`Self::unwound`]. `None`, running nothing, for a
    /// graph without a link.
    pub(super) fn with_link<R>(
        &mut self,
        body: impl FnOnce(&mut Self, &mut AudioLink, &mut Option<Box<Publication>>) -> R,
    ) -> Option<R> {
        let link = self.link.take()?;
        let body =
            |graph: &mut Self, (link, in_hand): &mut (AudioLink, _)| body(graph, link, in_hand);
        let restore = |graph: &mut Self, (link, in_hand): (AudioLink, Option<_>)| {
            if let Some(publication) = in_hand {
                graph.keep(publication);
            }
            graph.link = Some(link);
        };
        Some(self.restoring((link, None), body, restore))
    }

    /// Keeps a publication a render held as it panicked, for teardown. A
    /// render holds one at a time, so a second never comes; were one to,
    /// it is leaked rather than freed here.
    fn keep(&mut self, publication: Box<Publication>) {
        match self.unwound {
            None => self.unwound = Some(publication),
            Some(_) => std::mem::forget(publication),
        }
    }

    /// Fails the graph as a panic leaves a block: closes its request queue,
    /// so every later submission is refused, and has every later block
    /// render silence. Lock-free, and neither allocates nor frees.
    pub(super) fn fail(&mut self) {
        self.failed = true;
        if let Some(drain) = &self.requests {
            drain.close();
        }
    }
}
