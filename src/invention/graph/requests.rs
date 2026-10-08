//! Control requests on the audio thread: intake, generation mapping, and
//! applying each one exactly at its sample.
//!
//! # Ordering against publications
//!
//! This mirrors [`InputWrite`](super::InputWrite)s exactly. A control thread
//! resolves a request's target against the publisher's mirror (see
//! `Publisher::control_target`) and submits it while still holding the
//! publisher lock, then calls `note_written`. So every request tagged with
//! generation `g` is published in the queue before publication `g + 1` is
//! put in the mailbox, under that same lock; the audio thread's take of
//! `g + 1` acquires that put before it pops. Requests from the audio
//! thread itself (automation, later) are tagged with the installed
//! generation. In the block that installs a publication, then:
//!
//! - every request still tagged with an older generation (the one
//!   installed before, or one folded into the publication installing) is
//!   already in the queue, in front of any tagged with a newer one (the
//!   publisher lock serializes the pushes), and the queue holds at most its
//!   capacity, so popping up to that capacity reaches them all;
//! - each one, and every pending entry with an older generation, is mapped
//!   through the retired publication's remaps exactly as an input write is
//!   (see `SignalGraph::dispose`): onto its module's new index, or refused
//!   when the module was removed or rebuilt;
//! - a request tagged with a newer generation (published, not yet
//!   installed) is held in the pending store until that install.
//!
//! # Two intake modes: back-pressure outside install blocks
//!
//! - **An install block** pops until the queue is empty (at most its
//!   capacity) and settles what does not fit as refused
//!   ([`Refusal::PendingFull`]). It must: this is the last block with the
//!   retired publication's remaps in hand, so a request left in the queue
//!   with an older generation could never be mapped again.
//! - **Every other block** pops only while the store has room, or while
//!   the request at the head replaces a waiting one (same target and
//!   sample: it needs no room, and last write wins), and leaves the rest
//!   in the queue. Producers then get a synchronous `QueueFull`,
//!   which never marks a generation written, so a fold keeps no remap for
//!   it. While installs are deferred (the retire channel is full and no
//!   publication is taken), every block is of this kind, so the generations
//!   with requests outstanding, and with them the folded publication's
//!   `Absorbed` remaps, stay bounded by the store and queue capacity, as
//!   input writes are bounded by their ring (FUG-292). Refusing instead
//!   would let every fold keep one more remap, without bound.
//!
//! FUG-309: A2's retirer adds a "has room" check before a request whose
//! payload may need retiring is popped. Outside an install block that just
//! stops popping. An install block cannot stop, so the integration must
//! keep the retired publication (taking no new one) until the queue is
//! drained. That cannot deadlock: the retire queue drains on a control
//! thread.
//!
//! # Sample accuracy by splitting the block
//!
//! `process_block` runs a block as segments bounded by the due times of
//! pending requests: at each segment start it applies every due request,
//! then processes up to the next due time. Every module so gets sample
//! accuracy without handling an offset itself, and a module's apply stays a
//! plain setter (FUG-310). The number of segments is bounded by distinct
//! due times in the block, which coalescing and the bounded store bound. A
//! block with nothing due is one segment, exactly as before.

use super::publication::{Disposition, Publication};
use super::SignalGraph;
#[cfg(test)]
use crate::control_request::RtValue;
use crate::control_request::{
    ControlIndex, Outcome, PendingStore, QueueConsumer, Refusal, Request, RequestValue, When,
};

/// Applies a request's value in tests (see [`SignalGraph::request_hook`]).
#[cfg(test)]
pub(crate) type RequestHook =
    fn(&mut SignalGraph, usize, ControlIndex, RtValue) -> Result<(), Refusal>;

/// The audio thread's end of a live graph's request channel.
pub(crate) struct RequestDrain {
    requests: QueueConsumer<Request>,
    /// Requests taken per block at most: the queue's capacity.
    pop_limit: usize,
    pub(crate) pending: PendingStore,
    /// The installed publication's generation, as of the last drain.
    installed: u64,
}

impl RequestDrain {
    /// Allocates the pending store: call it on a control thread. The queue
    /// must hold at most `pop_limit` requests.
    pub(crate) fn new(requests: QueueConsumer<Request>, pop_limit: usize, pending: usize) -> Self {
        Self {
            requests,
            pop_limit,
            pending: PendingStore::with_capacity(pending),
            installed: 0,
        }
    }
}

impl SignalGraph {
    /// Maps pending requests across this block's install (if any), then
    /// pops the queue into the pending store: all of it in an install
    /// block, otherwise only while the store has room (see the module
    /// docs). Runs in
    /// `drain_link` with the retired publication in hand. Allocation-,
    /// free- and lock-free.
    pub(super) fn drain_requests(&mut self, installed: u64, retired: Option<(u64, &Publication)>) {
        let Some(mut drain) = self.requests.take() else {
            return;
        };
        drain.installed = installed;
        let map = |graph: &Self, generation, module_idx| match graph
            .dispose(generation, module_idx, installed, retired)
        {
            Disposition::Apply(new) => Some(new),
            Disposition::Hold | Disposition::Drop => None,
        };
        if retired.is_some() {
            drain.pending.remap(installed, |target| {
                map(self, target.generation, target.module_idx)
            });
        }
        let now = self.current_sample;
        // Where a request lands: its target in the installed order (`None`
        // when its module went away) and its sample. A time already past
        // keeps its place in time order and applies late, at the first
        // segment start.
        let resolve = |graph: &Self, request: &Request| {
            let mut target = request.target;
            if target.generation <= installed {
                target.module_idx = map(graph, target.generation, target.module_idx)?;
                target.generation = installed;
            }
            let at = match request.when {
                When::Now => now,
                When::AtSample(sample) => sample,
            };
            Some((target, at))
        };
        for _ in 0..drain.pop_limit {
            // Back-pressure outside an install block, except for a request
            // that replaces a waiting one, which needs no room: last write
            // wins even when the store is saturated.
            if retired.is_none() && drain.pending.is_full() {
                let replaces = drain.requests.peek(|head| resolve(self, head));
                let replaces = replaces
                    .flatten()
                    .is_some_and(|(target, at)| drain.pending.coalesces_with(&target, at));
                if !replaces {
                    break;
                }
            }
            let Some(mut request) = drain.requests.pop() else {
                break;
            };
            match resolve(self, &request) {
                Some((target, at)) => {
                    request.target = target;
                    drain.pending.insert(request, at);
                }
                None => drain
                    .pending
                    .outcomes
                    .settle(request, Outcome::Refused(Refusal::TargetGone)),
            }
        }
        self.requests = Some(drain);
    }

    /// Applies every request due at the current sample and returns the
    /// length of the segment to process next: up to the next due request,
    /// at most `remaining` frames, at least one. Allocation-, free- and
    /// lock-free.
    pub(super) fn apply_due_requests(&mut self, remaining: usize) -> usize {
        let Some(mut drain) = self.requests.take() else {
            return remaining;
        };
        let now = self.current_sample;
        drain
            .pending
            .apply_due(now, drain.installed, |target, value| {
                self.apply_request(target.module_idx, target.control, value)
            });
        let next = drain.pending.next_due(drain.installed);
        self.requests = Some(drain);
        next.map_or(remaining, |at| (at - now).min(remaining as u64) as usize)
    }

    /// Applies `value` to `control` of the module at `module_idx`. No module
    /// accepts requests until FUG-310, so this refuses with
    /// [`Refusal::Unsupported`], except through a test's hook.
    fn apply_request(
        &mut self,
        module_idx: usize,
        control: ControlIndex,
        value: &RequestValue,
    ) -> Result<(), Refusal> {
        let value = match value {
            RequestValue::Value(value) => *value,
            RequestValue::Payload(payload) => match *payload {},
        };
        #[cfg(test)]
        if let Some(hook) = self.request_hook {
            return hook(self, module_idx, control, value);
        }
        let _ = (module_idx, control, value);
        Err(Refusal::Unsupported)
    }

    /// A [`RequestHook`] applying an `F32` request as an input write, with
    /// the control index as the input port's index.
    #[cfg(test)]
    pub(crate) fn request_as_input_write(
        &mut self,
        module_idx: usize,
        control: ControlIndex,
        value: RtValue,
    ) -> Result<(), Refusal> {
        let RtValue::F32(value) = value else {
            return Err(Refusal::Unsupported);
        };
        self.apply_input(module_idx, usize::from(control.0), value);
        Ok(())
    }
}
