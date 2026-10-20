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
//! request already due, those timed in beats included (so those submitted
//! before the edit act on the graph it replaces), pops the edit and hands
//! its publication to `drain_link`, which installs it, maps every pending
//! request and every watch (its target and its clock) through it exactly as
//! an input write is mapped (see `SignalGraph::dispose`: onto its module's
//! new index, or refused with [`Refusal::TargetGone`] when the module was
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
//! A request timed in beats needs room among the watches, which may never
//! free (a beat far off, a clock stopped): one of the installed generation
//! finding them full is refused (`Refusal::PendingFull`) rather than left
//! at the head of the queue, so it never blocks what follows, edits
//! included. One held for a newer generation needs room, as in the store.
//!
//! A clockless drain (a `NullBackend`'s) applies no back-pressure: what
//! fills its store (or its watches) waits for samples that never come,
//! so nothing would ever free the room. Every request it pops is placed
//! or, when there is no room for it, refused (`Refusal::PendingFull`):
//! none is left queued for want of room. Its blocks take up every edit
//! queued before them (see `publish::Settler`).
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
//! # Tiers
//!
//! Every request applies through [`SignalGraph::apply_ranked`]: one losing
//! to a higher-ranked write due at or after its own due sample is settled
//! [`Outcome::Overridden`] (a higher tier) or [`Outcome::Superseded`] (its
//! own tier, higher priority) and changes nothing, and one that applies
//! holds its control against lower tiers (see
//! `crate::control_request::hold`). The holds live in each module's cells,
//! so they follow the instance through an install: a surviving module
//! keeps them, a rebuilt or added one starts free, and a removed one's go
//! with it. A request submitted before an edit and due at its sample holds
//! the instance the edit replaces, so one submitted after it, on a rebuilt
//! module, is not held off by it. A reload's control changes on a
//! surviving module are a user's writes, and hold as any does.
//!
//! A ramp in beats runs on only while its control's hold is its own, so
//! any write taking the control ends it, and one applying at the same
//! sample and rank ends it directly (see `control_request::ramps`): automation's next
//! event replaces it.
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
    admits_hold, apply_declared, expired, take_automation, take_hold, Conditions, ControlIndex,
    ControlTarget, Outcome, OutcomeSender, PendingStore, QueueConsumer, Refusal, Request,
    RequestValue, RtValue, Standing, WatchSlots, Watches, When, CONDITION_CAPACITY,
};
use crate::payload::{Payload, Retirer};

/// Applies a request's value in tests (see [`SignalGraph::request_hook`]),
/// under [`PendingStore::apply_due`]'s contract.
#[cfg(test)]
pub(crate) type RequestHook =
    fn(&mut SignalGraph, usize, ControlIndex, RequestValue, &mut Retirer) -> Result<(), Refusal>;

/// Where a request taken from the queue waits: in the pending store for its
/// sample, among the watches on its clock, or among the conditions.
#[derive(Clone, Copy)]
enum Landing {
    At(ControlTarget, u64),
    Watch(ControlTarget, usize),
    Condition(ControlTarget),
}

/// The audio thread's end of a live graph's request channel.
pub(crate) struct RequestDrain {
    requests: QueueConsumer<Request>,
    /// Control requests taken per block at most: as many as the queue lets
    /// control threads queue, so one block settles at most a full store and
    /// a full queue. Edits do not count against it.
    pub(super) pop_limit: usize,
    pub(crate) pending: PendingStore,
    /// Requests timed in beats, and the ramps they start (see
    /// [`super::beats`]).
    pub(crate) watches: Watches,
    /// Requests timed by a condition (see [`super::conditions`]).
    pub(crate) conditions: Conditions,
    /// The installed publication's generation, as of the last drain.
    pub(super) installed: u64,
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

/// Gives an offline render's `graph` a request drain no request reaches,
/// built as [`link_requests`] builds one: it plays the modules' tables as
/// a live graph's drain does (see `super::beats`), so a render plays them
/// as live playback does. Allocates: control thread.
pub(crate) fn offline_requests(graph: &mut SignalGraph) {
    use crate::control_request::{outcome_channel, request_channel};
    use crate::payload::RetireQueue;
    let (sender, requests) = request_channel(1, Arc::clone(&graph.transport));
    let slots = sender.watch_slots();
    let (outcomes, _) = outcome_channel(1);
    let retirer = Retirer::new(RetireQueue::with_capacity(1), 1);
    graph.requests = Some(RequestDrain::new(
        requests,
        1,
        1,
        1,
        retirer,
        outcomes,
        Default::default(),
        slots,
    ));
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
    let slots = sender.watch_slots();
    let (outcomes, _) = outcome_channel(capacity);
    let retirer = Retirer::new(RetireQueue::with_capacity(capacity), capacity);
    graph.requests = Some(RequestDrain::new(
        requests,
        capacity,
        capacity,
        capacity,
        retirer,
        outcomes,
        Default::default(),
        slots,
    ));
    sender
}

impl RequestDrain {
    /// Allocates the pending store: call it on a control thread. The queue
    /// must hold at most `pop_limit` control requests; `retirer` takes every payload
    /// the audio side does not keep, and `outcomes` reports every request.
    /// `slots` are the request channel's ([`RequestSender::watch_slots`]):
    /// the conditions release theirs as they settle.
    ///
    /// [`RequestSender::watch_slots`]: crate::control_request::RequestSender::watch_slots
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        requests: QueueConsumer<Request>,
        pop_limit: usize,
        pending: usize,
        watches: usize,
        retirer: Retirer,
        outcomes: OutcomeSender,
        published: Arc<AtomicU64>,
        slots: Arc<WatchSlots>,
    ) -> Self {
        let mut store = PendingStore::new(pending, retirer, outcomes);
        // A condition waiting on a request submitted before it starts
        // waiting once it is taken: before then, every request ahead of it
        // can settle, and those taken behind it in the same block.
        store.remember(2 * pop_limit + pending + watches + CONDITION_CAPACITY);
        Self {
            requests,
            pop_limit,
            pending: store,
            watches: Watches::new(watches),
            conditions: Conditions::new(CONDITION_CAPACITY, slots),
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
    /// `can_install`, it first applies every request now due, those timed
    /// in beats included, so those submitted before the edit act on the
    /// graph it replaces, then pops the edit and returns its publication
    /// for `drain_link` to install.
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
        //
        // A request timed in beats waits among the watches instead, on its
        // clock, in its target's generation (see `super::beats`); one timed
        // by a condition among the conditions, its sources in that
        // generation too (see `super::conditions`).
        let resolve = |graph: &Self, request: &Request| {
            let mut target = request.target;
            let at = match request.when {
                When::Beat(beat) => {
                    Watches::check(request)?;
                    // Its clock is in its target's generation, as the
                    // target is: an install maps both (`remap_requests`).
                    let clock = beat.clock as usize;
                    if target.generation > installed {
                        return Ok(Some(Landing::Watch(target, clock)));
                    }
                    if target.generation < installed {
                        return Err(Refusal::TargetGone);
                    }
                    if expired(request, now) {
                        return Err(Refusal::Expired);
                    }
                    return Ok(Some(Landing::Watch(target, clock)));
                }
                When::Now => Some(now),
                When::AtSample(sample) => Some(sample),
                // Resolved by the sender; counted from here only for a
                // request that never went through it.
                When::AfterSamples(samples) => Some(now.saturating_add(samples)),
                // Left for this thread when the clock was not yet anchored
                // at submission: it is now, unless there is no clock.
                When::AtTime(time) => graph.transport.sample_at(time),
                When::Condition(_) => {
                    Conditions::check(request)?;
                    if target.generation < installed {
                        return Err(Refusal::TargetGone);
                    }
                    if target.generation == installed && expired(request, now) {
                        return Err(Refusal::Expired);
                    }
                    return Ok(Some(Landing::Condition(target)));
                }
            };
            if target.generation > installed {
                return Ok(at.map(|at| Landing::At(target, at)));
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
            Ok(Some(Landing::At(target, at)))
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
            let (needs_room, full) = match landing {
                Ok(Landing::At(target, at)) => (
                    !(replaces && drain.pending.coalesces_with(&target, at, event)),
                    drain.pending.is_full(),
                ),
                // A watch may wait without end (a repeating grid, a far
                // beat, a stopped clock), so a full store refuses one of
                // the installed generation rather than hold the queue
                // behind it; one held for a newer generation waits, until
                // that install.
                Ok(Landing::Watch(target, _)) => {
                    (target.generation > installed, drain.watches.is_full())
                }
                // A condition may never be met: as a watch.
                Ok(Landing::Condition(target)) => {
                    (target.generation > installed, drain.conditions.is_full())
                }
                Err(_) => (false, false),
            };
            if !drain.clockless && full && needs_room && !edit_waiting {
                break;
            }
            let Some(mut request) = drain.requests.pop() else {
                break;
            };
            *budget -= 1;
            drain.pending.outcomes.reserve(&request);
            match landing {
                Ok(Landing::At(target, at)) => {
                    request.target = target;
                    if replaces {
                        drain.pending.insert(request, at);
                    } else {
                        drain.pending.insert_beside(request, at);
                    }
                }
                Ok(Landing::Watch(target, clock)) => {
                    request.target = target;
                    let outcomes = &mut drain.pending.outcomes;
                    drain.watches.insert(request, clock, outcomes);
                }
                Ok(Landing::Condition(target)) => {
                    request.target = target;
                    let outcomes = &mut drain.pending.outcomes;
                    drain.conditions.insert(request, outcomes);
                }
                // A condition's settles through the table, releasing its
                // slot.
                Err(refusal) if matches!(request.when, When::Condition(_)) => {
                    let outcomes = &mut drain.pending.outcomes;
                    drain.conditions.settle(request, refusal.into(), outcomes);
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
            // Beat-timed requests due now included; the segment start
            // after the install looks again for the next one.
            let _ = self.apply_due(&mut drain, now);
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
        let map = |graph: &Self, generation, module_idx| match graph
            .dispose(generation, module_idx, installed, retired)
        {
            Disposition::Apply(new) => Some(new),
            Disposition::Hold | Disposition::Drop => None,
        };
        drain.pending.remap(installed, now, |target| {
            map(self, target.generation, target.module_idx)
        });
        // A watch's clock maps with its target: a clock removed or rebuilt
        // refuses it (`Refusal::TimelineGone`).
        let outcomes = &mut drain.pending.outcomes;
        drain
            .watches
            .remap(installed, outcomes, |generation, module_idx| {
                map(self, generation, module_idx)
            });
        // A condition's sources map with its target: one removed or rebuilt
        // refuses it (`Refusal::SourceGone`).
        drain
            .conditions
            .remap(installed, outcomes, |generation, module_idx| {
                map(self, generation, module_idx)
            });
        self.requests = Some(drain);
    }

    /// Applies every request in `drain` due at `now`, those timed in beats
    /// included, and returns how many samples until the next may be due.
    /// Allocation-, free- and lock-free.
    fn apply_due(&mut self, drain: &mut RequestDrain, now: u64) -> Option<u64> {
        // New beat spans count from the clocks as they are before a
        // request timed in samples (a reset) moves them. An edit splits
        // the sample: what was submitted before it applies before it
        // installs, so a span submitted after the edit counts from after
        // such a reset.
        drain.watches.arm(drain.installed, self);
        // Conditions start reading here and read controls; one met in a
        // segment its target had already computed applies now, late.
        let outcomes = &mut drain.pending.outcomes;
        drain
            .conditions
            .start(now, drain.installed, &*self, outcomes);
        self.apply_conditions(drain, None, now);
        let ramps = &mut drain.watches.ramps;
        drain
            .pending
            .apply_due(now, drain.installed, |target, standing, value, retirer| {
                let applied = self.apply_ranked(target, standing, value, retirer);
                // At a ramp's own rank and sample, it leaves the hold as the
                // ramp took it: end the ramp here.
                if applied.is_ok() {
                    ramps.supersede(target, &*self);
                }
                applied
            });
        // What those requests applying met applies with them, before beats
        // are placed: it may move a clock (a tempo). So may what the beats
        // applying meet, so beats are placed again until none applies.
        let beats = loop {
            let chains = &drain.pending.outcomes.chains;
            drain.conditions.chain(drain.installed, chains);
            self.apply_conditions(drain, None, now);
            let outcomes = &mut drain.pending.outcomes;
            let beats = self.watch_beats(&mut drain.watches, outcomes, drain.installed);
            let waiting = drain.conditions.len();
            if waiting == 0 {
                break beats;
            }
            let chains = &drain.pending.outcomes.chains;
            drain.conditions.chain(drain.installed, chains);
            self.apply_conditions(drain, None, now);
            if drain.conditions.len() == waiting {
                break beats;
            }
        };
        let next = drain.pending.next_due(drain.installed).map(|at| at - now);
        match beats {
            Some(wait) => Some(next.map_or(wait, |next| next.min(wait))),
            None => next,
        }
    }

    /// Applies every request due at the current sample, those timed in
    /// beats included, and returns the length of the segment to process
    /// next: up to the next due request, at most `remaining` frames, at
    /// least one. Allocation-, free- and lock-free.
    pub(super) fn apply_due_requests(&mut self, remaining: usize) -> usize {
        let Some(mut drain) = self.requests.take() else {
            return remaining;
        };
        let now = self.current_sample;
        let next = self.apply_due(&mut drain, now);
        self.requests = Some(drain);
        next.map_or(remaining, |next| next.min(remaining as u64) as usize)
    }

    /// Applies a request of `standing` (none for an event) as
    /// [`Self::apply_request`] does, unless a higher-ranked write due at or
    /// after its due sample holds its control: then it reports what it
    /// lost as and changes nothing (a payload is retired). Once applied,
    /// it holds the control (see `crate::control_request::hold`).
    /// Allocation-, free- and lock-free.
    pub(super) fn apply_ranked(
        &mut self,
        target: &ControlTarget,
        standing: Option<Standing>,
        value: RequestValue,
        retirer: &mut Retirer,
    ) -> Result<(), Outcome> {
        let index = target.module_idx;
        if let (Some(standing), Some((_, instance))) = (standing, self.modules.get_index(index)) {
            if let Err(lost) = admits_hold(instance.module(), target.control, standing) {
                if let RequestValue::Payload(payload) | RequestValue::Edit(payload) = value {
                    retirer.retire(payload);
                }
                return Err(lost);
            }
        }
        self.apply_request(target.module_idx, target.control, value, retirer)?;
        if let (Some(standing), Some((_, instance))) = (standing, self.modules.get_index(index)) {
            take_hold(instance.module(), target.control, standing);
        }
        Ok(())
    }

    /// Applies `value` to `control` of the module at `module_idx`, under
    /// [`PendingStore::apply_due`]'s contract: a value through the module's
    /// [`apply`](crate::Module::apply), publishing what the control then
    /// holds (see [`apply_declared`]); a payload through its
    /// [`apply_payload`](crate::Module::apply_payload), retiring whichever
    /// value it does not keep. A test's hook replaces all of it.
    pub(super) fn apply_request(
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
                let Some((_, instance)) = self.modules.get_index_mut(module_idx) else {
                    retirer.retire(payload);
                    return Err(Refusal::TargetGone);
                };
                match instance.module_mut().apply_payload(control, payload) {
                    Ok(replaced) => {
                        retirer.retire(replaced);
                        Ok(())
                    }
                    Err((refusal, payload)) => {
                        retirer.retire(payload);
                        Err(refusal)
                    }
                }
            }
            // An edit is installed by the drain, never applied.
            RequestValue::Edit(payload) => {
                retirer.retire(payload);
                Err(Refusal::Unsupported)
            }
        }
    }

    /// Applies `value` to declared control `control` of module `module_id`
    /// at once: an offline render's control write, made under the lock its
    /// renders take (see [`crate::invention::declared::Route`]). A user's,
    /// it holds the control against lower tiers as a request does.
    pub(crate) fn apply_control(
        &mut self,
        module_id: &str,
        control: ControlIndex,
        value: RtValue,
    ) -> Result<(), Refusal> {
        self.apply_control_due(module_id, control, value, self.current_sample)
    }

    /// [`Self::apply_control`] for a write due at sample `due`, which it
    /// holds from, as a request applied late does: a replayed request.
    pub(crate) fn apply_control_due(
        &mut self,
        module_id: &str,
        control: ControlIndex,
        value: RtValue,
        due: u64,
    ) -> Result<(), Refusal> {
        let instance = self.modules.get_mut(module_id).ok_or(Refusal::TargetGone)?;
        take_automation(instance.module_mut());
        apply_declared(instance.module_mut(), control, value)?;
        take_hold(instance.module(), control, Standing::user(due));
        Ok(())
    }

    /// Keeps `payload` as declared payload control `control` of module
    /// `module_id` at once, as [`Self::apply_control`] applies a value,
    /// holding it as that does. Control thread: the value it replaces, or a
    /// refused payload, is dropped here.
    pub(crate) fn apply_payload(
        &mut self,
        module_id: &str,
        control: ControlIndex,
        payload: Payload,
    ) -> Result<(), Refusal> {
        let instance = self.modules.get_mut(module_id).ok_or(Refusal::TargetGone)?;
        instance
            .module_mut()
            .apply_payload(control, payload)
            .map(drop)
            .map_err(|(refusal, _)| refusal)?;
        take_hold(
            instance.module(),
            control,
            Standing::user(self.current_sample),
        );
        Ok(())
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
