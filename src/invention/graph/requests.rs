//! Control requests on the audio thread: intake, generation mapping, and
//! applying each one exactly at its sample.
//!
//! # One order with edits
//!
//! Structural edits ride the same queue: `Publisher::publish` submits the
//! prepared publication as a [`RequestValue::Edit`], `when = Now`. A
//! control thread resolves a request's target against the publisher's
//! mirror (see `Publisher::control_target`) and submits it while still
//! holding the publisher lock, as every edit is submitted. So the queue
//! holds requests and edits in submit order, which is generation order:
//! every request tagged with generation `g` is in front of the edit that
//! publishes `g + 1`, and every one behind it is tagged `g + 1`. The edit's
//! push releases, and the drain's pop acquires, everything queued before
//! it, input writes included. (A producer on the audio thread, such as
//! automation later, cannot take that lock; it must insert into the pending
//! store directly instead.)
//!
//! The drain pops in that order. At an edit at the head it applies every
//! request already due (so those submitted before the edit act on the graph
//! it replaces), pops the edit and hands its publication to `drain_link`,
//! which installs it, maps every pending request through it exactly as an
//! input write is mapped (see `SignalGraph::dispose`: onto its module's new
//! index, or refused with [`Refusal::TargetGone`] when the module was
//! removed or rebuilt), retires it, and drains on. Several edits can
//! install in one block, in order. While the link holds a retirement (its
//! retire ring was full), the edit stays at the head and everything behind
//! it waits in the queue; no request is ever tagged with a generation the
//! drain has not installed, so none is held for one.
//!
//! An immediate edit sends no outcome: its acknowledgement is its commit,
//! which no audio-side condition can undo.
//!
//! # Back-pressure, and edits never starve
//!
//! A request is popped only while the store has room, or while the request
//! at the head replaces a waiting one (same target and sample, and not an
//! event: it needs no room, and last write wins) or is refused (its module
//! gone or its ttl run out: settled at once, it needs no room either);
//! otherwise it and everything behind it stay queued, and producers get a
//! synchronous `QueueFull` once the queue fills. Two things keep that from
//! shutting out an edit:
//!
//! - control requests may fill only part of the queue
//!   (`RequestSender::reserving`), leaving the rest for edits, so an edit
//!   always finds a slot while installs keep up;
//! - while an edit waits behind a full store and could install (the
//!   publisher's `published` generation is ahead of the installed one), the
//!   requests in front of it are popped anyway and those finding no room
//!   settled as refused ([`Refusal::PendingFull`]). The first edit waiting
//!   then installs this block: no more control requests than the block's
//!   budget can be queued in front of it. One behind it may wait a block
//!   once the budget is spent; a payload without retire room stops intake
//!   ahead of any edit (below).
//!
//! A clockless drain (a `NullBackend`'s) applies no back-pressure: what
//! fills its store waits for samples that never come, so nothing would
//! ever free the room. Every request it pops is placed or, when the store
//! is full, refused (`Refusal::PendingFull`): none is left queued for want
//! of room. Its blocks take up every edit queued before them (see
//! `publish::Settler`).
//!
//! # Retire room: payloads stop the intake
//!
//! The drain flushes its retirer at every block start, and takes a request
//! carrying a payload only while the retirer has room for it on top of
//! every pending payload's reservation (see
//! [`Outcomes`](crate::control_request::Outcomes)). Without room it stops
//! popping: the request and everything behind it, edits included, stay
//! queued, in order, and the next block retries once a control thread has
//! drained the retire queue.
//!
//! The retirer is sized to cover every pending request and a full queue
//! (see `Publisher::link`), so reservations alone never exhaust it: room
//! runs out only while retirements wait for the reclaimer, which drains on
//! a control thread every few tens of ms. An edit behind a payload
//! therefore waits for the reclaimer, never for a request's sample, and
//! cannot deadlock.
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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
    /// Control requests taken per block at most: as many as the queue lets
    /// control threads queue, so one block settles at most a full store and
    /// a full queue. Edits do not count against it.
    pub(super) pop_limit: usize,
    pub(crate) pending: PendingStore,
    /// The installed publication's generation, as of the last drain.
    installed: u64,
    /// No time passes (a `NullBackend`): a request that finds the store
    /// full is refused ([`Refusal::PendingFull`]) rather than left queued,
    /// since what fills the store waits for samples that never come.
    pub(crate) clockless: bool,
    /// The newest generation the publisher has queued an edit for. Newer
    /// than `installed` means an edit is waiting in the queue, so requests
    /// ahead of it are taken even into a full store (see the module docs).
    /// Only a hint for when to stop holding back, so `Relaxed` is enough:
    /// a stale value holds back one more block.
    published: Arc<AtomicU64>,
    /// Run as the drain is dropped, before its queue: closes the link to
    /// further submissions (see `Publisher::close`).
    pub(crate) on_drop: Option<Box<dyn FnOnce() + Send>>,
}

/// Gives `graph` a request drain for `capacity` requests, as
/// `Publisher::link` does, and returns the sender edits and requests go
/// through: for tests that link a graph by hand.
#[cfg(test)]
pub(crate) fn link_requests(
    graph: &mut SignalGraph,
    capacity: usize,
) -> crate::control_request::RequestSender {
    use crate::control_request::{outcome_channel, request_channel};
    use crate::payload::RetireQueue;
    let (sender, requests) = request_channel(capacity, std::sync::Arc::clone(&graph.transport));
    let (outcomes, _) = outcome_channel(capacity);
    let retirer = Retirer::new(RetireQueue::with_capacity(capacity), capacity);
    graph.requests = Some(RequestDrain::new(
        requests,
        capacity,
        capacity,
        retirer,
        outcomes,
        Default::default(),
    ));
    sender
}

impl RequestDrain {
    /// Allocates the pending store: call it on a control thread. The queue
    /// must hold at most `pop_limit` control requests; `retirer` takes every payload
    /// the audio side does not keep, and `outcomes` reports every request.
    pub(crate) fn new(
        requests: QueueConsumer<Request>,
        pop_limit: usize,
        pending: usize,
        retirer: Retirer,
        outcomes: OutcomeSender,
        published: Arc<AtomicU64>,
    ) -> Self {
        Self {
            requests,
            pop_limit,
            pending: PendingStore::new(pending, retirer, outcomes),
            installed: 0,
            clockless: false,
            published,
            on_drop: None,
        }
    }
}

impl Drop for RequestDrain {
    /// Dropped with the graph, on a control thread once the audio has
    /// stopped; the queue then drops whatever is left in it. A render
    /// takes the drain out of the graph while requests apply, so a panic
    /// there drops it while unwinding, on the rendering thread and maybe
    /// under the publisher (a settle): it then neither locks nor frees, and
    /// leaks the hook.
    fn drop(&mut self) {
        let Some(close) = self.on_drop.take() else {
            return;
        };
        if std::thread::panicking() {
            std::mem::forget(close);
            return;
        }
        close();
    }
}

impl SignalGraph {
    /// Pops the queue into the pending store, in order, while the store has
    /// room and the retirer has room for a payload (see the module docs),
    /// taking at most `budget` requests. Stops at an edit: when
    /// `can_install`, it first applies every request now due, so those
    /// submitted before the edit act on the graph it replaces, then pops
    /// the edit and returns its publication for `drain_link` to install.
    /// Runs in `drain_link`. Allocation-, free- and lock-free.
    pub(super) fn drain_requests(
        &mut self,
        installed: u64,
        can_install: bool,
        budget: &mut usize,
    ) -> Option<Box<Publication>> {
        let mut drain = self.requests.take()?;
        drain.pending.outcomes.retirer.flush();
        drain.installed = installed;
        let now = self.current_sample;
        // Where a request lands: its target in the installed order and its
        // sample, or why it is refused: its module went away, or it could
        // only apply after its `expires` sample. A time already past keeps
        // its place in time order and applies late, at the first segment
        // start. A request for a generation not yet installed is held
        // whatever its expiry, and refused once due after the install
        // (`PendingStore::apply_due`). Until then it cannot know whether it
        // applies, so with a ttl it replaces nothing
        // (`PendingStore::insert_beside`), and a wall-clock time nothing can
        // place (no wall clock) stays queued until its generation installs
        // (`Ok(None)`), and is refused then. (An immediate edit is queued
        // ahead of every request for the generation it installs, so these
        // are held only once an edit can wait for its time.)
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
            };
            if target.generation > installed {
                return Ok(at.map(|at| (target, at)));
            }
            if target.generation < installed {
                // Defensive: a request queued behind an edit is never older
                // than the generation installed.
                return Err(Refusal::TargetGone);
            }
            target.generation = installed;
            let at = at.ok_or(Refusal::NoClock)?;
            if expired(request, at.max(now)) {
                return Err(Refusal::Expired);
            }
            Ok(Some((target, at)))
        };
        // An edit waiting behind a full store must not starve: the requests
        // ahead of it are taken, and those finding no room refused
        // (`Refusal::PendingFull`), so the first one installs this block.
        let edit_waiting = can_install && drain.published.load(Ordering::Relaxed) > installed;
        let mut edit = None;
        loop {
            let head = drain.requests.peek(|head| {
                let replaces = head.target.generation <= installed || head.expires.is_none();
                (
                    head.value.is_edit(),
                    head.value.is_payload(),
                    head.event,
                    replaces,
                    resolve(self, head),
                )
            });
            let Some((is_edit, payload, event, replaces, landing)) = head else {
                break;
            };
            if is_edit {
                if can_install {
                    edit = drain.requests.pop();
                }
                break;
            }
            if *budget == 0 {
                break;
            }
            // Unplaced and held: it waits in the queue, and in generation
            // order so does everything behind it.
            let Some(landing) = landing.transpose() else {
                break;
            };
            if payload && !drain.pending.outcomes.has_room_for_payload() {
                break;
            }
            // Back-pressure, except for a request that needs no room: one
            // refused (settled at once, so it never blocks what follows) or
            // one replacing a waiting request (last write wins even when
            // the store is saturated). A request held for a newer
            // generation still needs room.
            let needs_room = match landing {
                Ok((target, at)) => !(replaces && drain.pending.coalesces_with(&target, at, event)),
                Err(_) => false,
            };
            if !drain.clockless && drain.pending.is_full() && needs_room && !edit_waiting {
                break;
            }
            let Some(mut request) = drain.requests.pop() else {
                break;
            };
            *budget -= 1;
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
        // One order: what was submitted before the edit acts on the graph
        // it replaces, at this same sample.
        let publication = edit.map(|edit| {
            debug_assert_eq!(edit.target.generation, installed, "edits install in order");
            self.apply_due(&mut drain, now);
            Self::take_publication(&mut drain, edit)
        });
        self.requests = Some(drain);
        publication.flatten()
    }

    /// The publication an edit carries. An edit only ever carries one; any
    /// other payload is retired rather than dropped here.
    fn take_publication(drain: &mut RequestDrain, edit: Request) -> Option<Box<Publication>> {
        let RequestValue::Edit(payload) = edit.value else {
            unreachable!("popped as an edit");
        };
        match payload.take_owned::<Publication>() {
            Ok(publication) => Some(publication),
            Err(other) => {
                debug_assert!(false, "an edit carried something other than a publication");
                drain.pending.outcomes.retirer.retire(other);
                None
            }
        }
    }

    /// Maps every pending request resolved against the generation the
    /// install just retired (`retired`, with that generation) onto the
    /// `installed` order, or refuses it ([`Refusal::TargetGone`]) when its
    /// module was removed or rebuilt. Allocation-, free- and lock-free.
    pub(super) fn remap_requests(&mut self, installed: u64, retired: Option<(u64, &Publication)>) {
        let Some(mut drain) = self.requests.take() else {
            return;
        };
        drain.installed = installed;
        let now = self.current_sample;
        drain.pending.remap(installed, now, |target| {
            match self.dispose(target.generation, target.module_idx, installed, retired) {
                Disposition::Apply(new) => Some(new),
                Disposition::Hold | Disposition::Drop => None,
            }
        });
        self.requests = Some(drain);
    }

    /// Applies every request in `drain` due at `now`. Allocation-, free-
    /// and lock-free.
    fn apply_due(&mut self, drain: &mut RequestDrain, now: u64) {
        drain
            .pending
            .apply_due(now, drain.installed, |target, value, retirer| {
                self.apply_request(target.module_idx, target.control, value, retirer)
            });
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
        self.apply_due(&mut drain, now);
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
            RequestValue::Payload(payload) | RequestValue::Edit(payload) => {
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
            RequestValue::Payload(payload) | RequestValue::Edit(payload) => {
                retirer.retire(payload);
                return Err(Refusal::Unsupported);
            }
        };
        self.apply_input(module_idx, usize::from(control.0), value);
        Ok(())
    }
}
