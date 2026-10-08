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
//! `g + 1` acquires that put before it pops. Since every submission is
//! serialized by the publisher lock and the publisher's generation only
//! grows, the queue holds requests in generation order. (A producer on the
//! audio thread, such as automation later, cannot take that lock; it must
//! insert into the pending store directly instead.) In the block that
//! installs a publication, then:
//!
//! - every request still tagged with an older generation (the one
//!   installed before, or one folded into the publication installing) is
//!   already in the queue, in front of any tagged with the installed or a
//!   newer one, so popping until the head is not older reaches them all;
//! - each one, and every pending entry with an older generation, is mapped
//!   through the retired publication's remaps exactly as an input write is
//!   (see `SignalGraph::dispose`): onto its module's new index, or refused
//!   when the module was removed or rebuilt;
//! - a request tagged with a newer generation (published, not yet
//!   installed) is held in the pending store until that install.
//!
//! # Two intake modes: back-pressure outside install blocks
//!
//! - **An install block** pops every request older than the installed
//!   generation and settles what does not fit as refused
//!   ([`Refusal::PendingFull`]). It must: this is the last block with the
//!   retired publication's remaps in hand, so a request left in the queue
//!   with an older generation could never be mapped again.
//! - **Every other request** (all of them outside an install block) is
//!   popped only while the store has room, or while
//!   the request at the head replaces a waiting one (same target and
//!   sample: it needs no room, and last write wins), and leaves the rest
//!   in the queue. Producers then get a synchronous `QueueFull`,
//!   which never marks a generation written, so a fold keeps no remap for
//!   it. While installs are deferred (the retire ring is full and no
//!   publication is taken), every block is of this kind, so the generations
//!   with requests outstanding, and with them the folded publication's
//!   `Absorbed` remaps, stay bounded by the store and queue capacity, as
//!   input writes are bounded by their ring (FUG-292). Refusing instead
//!   would let every fold keep one more remap, without bound.
//!
//! # Retire room: payloads stop the intake
//!
//! The drain flushes its retirer at every block start, and takes a request
//! carrying a payload only while the retirer has room for it on top of
//! every pending payload's reservation (see
//! [`Outcomes`](crate::control_request::Outcomes)). Without room it stops
//! popping: the request and everything behind it stay queued, in order,
//! and the next block retries once a control thread has drained the retire
//! queue. Outside an install that is all. An install block cannot leave
//! older-generation requests behind, so when it stops at one the link keeps
//! the retired publication, taking no new one, and every following block
//! goes on popping with its remaps in hand until the head is no longer
//! older (or the queue is empty); only then does the publication go back.
//!
//! The retirer is sized to cover every pending request and a full queue
//! (see `Publisher::link`), so reservations alone never exhaust it: room
//! runs out only while retirements wait for the reclaimer, which drains on
//! a control thread every few tens of ms. A kept publication therefore
//! waits for the reclaimer, never for a request's sample or a later
//! install, and cannot deadlock.
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
    ControlIndex, Outcome, OutcomeSender, PendingStore, QueueConsumer, Refusal, Request, RequestValue, When,
};
use crate::payload::Retirer;

/// Applies a request's value in tests (see [`SignalGraph::request_hook`]),
/// under [`PendingStore::apply_due`]'s contract.
#[cfg(test)]
pub(crate) type RequestHook =
    fn(&mut SignalGraph, usize, ControlIndex, RequestValue, &mut Retirer) -> Result<(), Refusal>;

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
    /// must hold at most `pop_limit` requests; `retirer` takes every payload
    /// the audio side does not keep, and `outcomes` reports every request.
    pub(crate) fn new(
        requests: QueueConsumer<Request>,
        pop_limit: usize,
        pending: usize,
        retirer: Retirer,
        outcomes: OutcomeSender,
    ) -> Self {
        Self {
            requests,
            pop_limit,
            pending: PendingStore::new(pending, retirer, outcomes),
            installed: 0,
        }
    }
}

impl SignalGraph {
    /// Maps pending requests across the install whose retired publication
    /// is in hand (if any), then pops the queue into the pending store:
    /// every request older than `installed`, and the rest only while the
    /// store has room, all only while the retirer has room for a payload
    /// (see the module docs). Runs in `drain_link`. Returns false when a
    /// request older than `installed` is still queued, so the caller must
    /// keep `retired` for the next block. Allocation-, free- and lock-free.
    pub(super) fn drain_requests(
        &mut self,
        installed: u64,
        retired: Option<(u64, &Publication)>,
    ) -> bool {
        let Some(mut drain) = self.requests.take() else {
            return true;
        };
        drain.pending.outcomes.retirer.flush();
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
        let mut mapped = true;
        for _ in 0..drain.pop_limit {
            let head = drain.requests.peek(|head| {
                let older = head.target.generation < installed;
                (head.value.is_payload(), older, resolve(self, head))
            });
            let Some((payload, older, landing)) = head else {
                break;
            };
            // An install must pop every older request while it holds the
            // remaps; any it cannot keep the retired publication in hand.
            let owed = retired.is_some() && older;
            if payload && !drain.pending.outcomes.has_room_for_payload() {
                mapped = !owed;
                break;
            }
            // Back-pressure for the rest, except for a request that
            // replaces a waiting one, which needs no room: last write wins
            // even when the store is saturated.
            if !owed
                && drain.pending.is_full()
                && !landing.is_some_and(|(target, at)| drain.pending.coalesces_with(&target, at))
            {
                break;
            }
            let Some(mut request) = drain.requests.pop() else {
                break;
            };
            drain.pending.outcomes.reserve(&request);
            match landing {
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
        mapped
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
            .apply_due(now, drain.installed, |target, value, retirer| {
                self.apply_request(target.module_idx, target.control, value, retirer)
            });
        let next = drain.pending.next_due(drain.installed);
        self.requests = Some(drain);
        next.map_or(remaining, |at| (at - now).min(remaining as u64) as usize)
    }

    /// Applies `value` to `control` of the module at `module_idx`, under
    /// [`PendingStore::apply_due`]'s contract. No module accepts requests
    /// until FUG-310, so this refuses with [`Refusal::Unsupported`],
    /// retiring a payload, except through a test's hook.
    fn apply_request(
        &mut self,
        module_idx: usize,
        control: ControlIndex,
        value: RequestValue,
        retirer: &mut Retirer,
    ) -> Result<(), Refusal> {
        #[cfg(test)]
        if let Some(hook) = self.request_hook {
            return hook(self, module_idx, control, value, retirer);
        }
        let _ = (module_idx, control);
        if let RequestValue::Payload(payload) = value {
            retirer.retire(payload);
        }
        Err(Refusal::Unsupported)
    }

    /// A [`RequestHook`] applying an `F32` request as an input write, with
    /// the control index as the input port's index.
    #[cfg(test)]
    pub(crate) fn request_as_input_write(
        &mut self,
        module_idx: usize,
        control: ControlIndex,
        value: RequestValue,
        retirer: &mut Retirer,
    ) -> Result<(), Refusal> {
        let value = match value {
            RequestValue::Value(RtValue::F32(value)) => value,
            RequestValue::Value(_) => return Err(Refusal::Unsupported),
            RequestValue::Payload(payload) => {
                retirer.retire(payload);
                return Err(Refusal::Unsupported);
            }
        };
        self.apply_input(module_idx, usize::from(control.0), value);
        Ok(())
    }
}
