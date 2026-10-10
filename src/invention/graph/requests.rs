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
//!   popped only while the store has room, or while the request at the
//!   head replaces a waiting one (same target and sample, and not an
//!   event: it needs no room, and last write wins) or is refused (its
//!   module gone or its ttl run out: settled at once, it needs no room
//!   either), and leaves the rest in the queue. Producers then get a
//!   synchronous `QueueFull`, which never marks a generation written, so a
//!   fold keeps no remap for it. While installs are deferred (the retire ring is full and no
//!   publication is taken), every block is of this kind, so the generations
//!   with requests outstanding, and with them the folded publication's
//!   `Absorbed` remaps, stay bounded by the store and queue capacity, as
//!   input writes are bounded by their ring (FUG-292). Refusing instead
//!   would let every fold keep one more remap, without bound.
//!
//! A clockless drain (a `NullBackend`'s) applies no back-pressure: what
//! fills its store waits for samples that never come, so nothing would
//! ever free the room. Every request it pops is placed or, when the store
//! is full, refused (`Refusal::PendingFull`): none is left queued for want
//! of room. Its blocks install every publication waiting (see
//! `publish::Settler`), so no remaps accumulate to bound.
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
//! plain setter. The number of segments is bounded by distinct
//! due times in the block, which coalescing and the bounded store bound. A
//! block with nothing due is one segment, exactly as before.

use super::publication::{Disposition, Publication};
use super::SignalGraph;
use crate::control_request::{
    apply_declared, expired, take_automation, ControlIndex, Outcome, OutcomeSender, PendingStore,
    QueueConsumer, Refusal, Request, RequestValue, RtValue, When,
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
    /// No time passes (a `NullBackend`): a request that finds the store
    /// full is refused ([`Refusal::PendingFull`]) rather than left queued,
    /// since what fills the store waits for samples that never come.
    pub(crate) clockless: bool,
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
            clockless: false,
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
        let now = self.current_sample;
        if retired.is_some() {
            drain.pending.remap(installed, now, |target| {
                map(self, target.generation, target.module_idx)
            });
        }
        // Where a request lands: its target in the installed order and its
        // sample, or why it is refused: its module went away, or it could
        // only apply after its `expires` sample. A time already past keeps
        // its place in time order and applies late, at the first segment
        // start. A request for a generation not yet installed is held
        // whatever its expiry, and refused once due after the install
        // (`PendingStore::apply_due`): refusing it here would free its room
        // and lift the back-pressure that bounds the folded remaps. Until
        // then it cannot know whether it applies, so with a ttl it replaces
        // nothing (`PendingStore::insert_beside`). For the same reason a
        // wall-clock time nothing can place (no wall clock) stays queued
        // until its generation installs (`Ok(None)`), and is refused then.
        let resolve = |graph: &Self, request: &Request| {
            let mut target = request.target;
            let at = match request.when {
                When::Now => Some(now),
                When::AtSample(sample) => Some(sample),
                // Resolved by the sender; counted from here only for a
                // request that never went through it.
                When::AfterSamples(samples) => Some(now.saturating_add(samples)),
                // Left for this thread when the clock was not yet anchored
                // at submission: it is now, unless there is no clock.
                When::AtTime(time) => graph.transport.sample_at(time),
                // Placed on its clock's beats with FUG-320's next step.
                When::Beat(_) => return Err(Refusal::Unsupported),
            };
            if target.generation > installed {
                return Ok(at.map(|at| (target, at)));
            }
            target.module_idx =
                map(graph, target.generation, target.module_idx).ok_or(Refusal::TargetGone)?;
            target.generation = installed;
            let at = at.ok_or(Refusal::NoClock)?;
            if expired(request, at.max(now)) {
                return Err(Refusal::Expired);
            }
            Ok(Some((target, at)))
        };
        let mut mapped = true;
        for _ in 0..drain.pop_limit {
            let head = drain.requests.peek(|head| {
                let older = head.target.generation < installed;
                let replaces = head.target.generation <= installed || head.expires.is_none();
                (
                    head.value.is_payload(),
                    head.event,
                    older,
                    replaces,
                    resolve(self, head),
                )
            });
            let Some((payload, event, older, replaces, landing)) = head else {
                break;
            };
            // Unplaced and held: it waits in the queue, and in generation
            // order so does everything behind it, none owed to this install.
            let Some(landing) = landing.transpose() else {
                break;
            };
            // An install must pop every older request while it holds the
            // remaps; any it cannot keep the retired publication in hand.
            let owed = retired.is_some() && older;
            if payload && !drain.pending.outcomes.has_room_for_payload() {
                mapped = !owed;
                break;
            }
            // Back-pressure for the rest, except for a request that needs
            // no room: one refused (settled at once, so it never blocks
            // what follows) or one replacing a waiting request (last write
            // wins even when the store is saturated). Only an installed or
            // older generation is refused here, so a request held for a
            // newer one still needs room and the folded remaps stay bounded.
            let needs_room = match landing {
                Ok((target, at)) => !(replaces && drain.pending.coalesces_with(&target, at, event)),
                Err(_) => false,
            };
            if !owed && !drain.clockless && drain.pending.is_full() && needs_room {
                break;
            }
            let Some(mut request) = drain.requests.pop() else {
                break;
            };
            drain.pending.outcomes.reserve(&request);
            match landing {
                Ok((target, at)) => {
                    request.target = target;
                    if replaces {
                        drain.pending.insert(request, at);
                    } else {
                        drain.pending.insert_beside(request, at);
                    }
                }
                Err(refusal) => drain
                    .pending
                    .outcomes
                    .settle(request, Outcome::Refused(refusal)),
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
    /// [`PendingStore::apply_due`]'s contract: a value through the module's
    /// [`apply`](crate::Module::apply), publishing what the control then
    /// holds (see [`apply_declared`]). No module takes a payload yet, so one
    /// is retired and refused with [`Refusal::Unsupported`]. A test's hook
    /// replaces all of it.
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
        match value {
            RequestValue::Value(value) => {
                let (_, instance) = self
                    .modules
                    .get_index_mut(module_idx)
                    .ok_or(Refusal::TargetGone)?;
                // Automation still waiting was written before this sample.
                take_automation(instance.module_mut());
                apply_declared(instance.module_mut(), control, value)
            }
            RequestValue::Payload(payload) => {
                retirer.retire(payload);
                Err(Refusal::Unsupported)
            }
        }
    }

    /// Applies `value` to declared control `control` of module `module_id`
    /// at once: an offline render's control write, made under the lock its
    /// renders take (see [`crate::invention::declared::Route`]).
    pub(crate) fn apply_control(
        &mut self,
        module_id: &str,
        control: ControlIndex,
        value: RtValue,
    ) -> Result<(), Refusal> {
        let instance = self.modules.get_mut(module_id).ok_or(Refusal::TargetGone)?;
        take_automation(instance.module_mut());
        apply_declared(instance.module_mut(), control, value)
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
