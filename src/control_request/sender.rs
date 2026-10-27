//! Submitting requests to the audio thread.

use super::conditions::WatchSlots;
use super::event::EventCounter;
use super::queue::{bounded, QueueConsumer, QueueProducer};
use super::request::{Request, RequestId, When};
use super::sync::{debug_assert_control_thread, Arc, AtomicU64, Ordering};
use super::transport::Transport;

/// Creates a request channel holding up to `capacity` requests (rounded as
/// [`bounded`] rounds it), timing relative requests against `transport`.
/// The consumer goes to the audio thread's drain. Allocates once: call it
/// on a control thread.
pub(crate) fn request_channel(
    capacity: usize,
    transport: Arc<Transport>,
) -> (RequestSender, QueueConsumer<Request>) {
    debug_assert_control_thread("request_channel");
    let (queue, consumer) = bounded(capacity);
    let shared = Arc::new(SenderShared {
        next_id: AtomicU64::new(1),
        overflows: EventCounter::new(),
        transport,
        watches: Arc::new(WatchSlots::default()),
        reserved: Arc::new(ReservedQueued::default()),
    });
    let sender = RequestSender {
        queue,
        shared,
        reserve: 0,
    };
    (sender, consumer)
}

/// The queue had no room: the request comes back unsent, so a payload it
/// carries is never dropped by the queue.
#[derive(Debug)]
pub(crate) struct QueueFull(pub(crate) Request);

/// Submits requests to one channel. Clone it for each producer.
///
/// Dropping the last handle of a channel (this, or its consumer) frees the
/// queue, so the audio thread must never hold the last one.
#[derive(Clone)]
pub(crate) struct RequestSender {
    queue: QueueProducer<Request>,
    shared: Arc<SenderShared>,
    /// Slots this handle leaves free for handles that may use them (see
    /// [`Self::reserving`]).
    reserve: u64,
}

struct SenderShared {
    /// The next request id. `Relaxed` is enough: the counter only has to
    /// hand out distinct values, which RMW atomicity guarantees.
    next_id: AtomicU64,
    overflows: EventCounter,
    transport: Arc<Transport>,
    /// Requests timed by a condition in flight, released by the drain as
    /// each settles.
    watches: Arc<WatchSlots>,
    reserved: Arc<ReservedQueued>,
}

/// Reserved requests ([`Request::reserved`]) queued and taken, so the drain
/// knows one waits behind the head of a queue it would otherwise leave
/// alone for a full store, and takes what is ahead of it, as it does for an
/// edit (see `graph::requests`).
#[derive(Debug, Default)]
pub(crate) struct ReservedQueued {
    queued: AtomicU64,
    /// Written by the drain alone.
    taken: AtomicU64,
}

impl ReservedQueued {
    /// The drain took one.
    pub(crate) fn took(&self) {
        let taken = self.taken.load(Ordering::Relaxed);
        self.taken.store(taken + 1, Ordering::Relaxed);
    }

    /// Whether one waits in the queue. Seen before the request it counts,
    /// the drain takes what is ahead of it a block early; seen late, it
    /// waits a block.
    pub(crate) fn waiting(&self) -> bool {
        self.queued.load(Ordering::Acquire) > self.taken.load(Ordering::Relaxed)
    }
}

impl RequestSender {
    /// A handle to the same channel that refuses a request ([`QueueFull`])
    /// unless `reserve` more slots stay free after it, for handles without
    /// one: a live graph keeps them for structural edits, so a flood of
    /// control requests never shuts an edit out. Exact while submissions
    /// are serialized, as a live graph's are (under its publisher).
    pub(crate) fn reserving(&self, reserve: u64) -> Self {
        Self {
            reserve,
            ..self.clone()
        }
    }

    /// Assigns `request` the channel's next id, resolves its relative times
    /// and queues it. Lock-free and allocation-free from any thread,
    /// including the audio thread.
    ///
    /// [`When::AfterSamples`] and the `ttl` count from the transport's
    /// published count ([`Transport::rendered`]) as of this call: they
    /// become [`When::AtSample`] and the request's `expires` sample.
    /// [`When::AtTime`] becomes the sample heard then
    /// ([`Transport::sample_at`]) once the clock is anchored; before that
    /// the audio thread places it. They are resolved once, so a request
    /// handed back in [`QueueFull`] keeps its times when submitted again.
    ///
    /// # Errors
    ///
    /// [`QueueFull`] with the request (id assigned) when the queue has no
    /// room, or is [`closed`](Self::closed); the overflow is also counted
    /// in [`overflows`](Self::overflows).
    // The request comes back by value, as large as it is (a condition makes
    // it 176 bytes), so a full queue never allocates.
    #[allow(clippy::result_large_err)]
    pub(crate) fn submit(&self, request: Request) -> Result<RequestId, QueueFull> {
        self.submit_as(self.take_id(), request)
    }

    /// The channel's next id, for a request submitted later with
    /// [`Self::submit_as`], so others can wait on it first.
    pub(crate) fn take_id(&self) -> RequestId {
        RequestId(self.shared.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// [`Self::submit`] with an id from [`Self::take_id`].
    #[allow(clippy::result_large_err)]
    pub(crate) fn submit_as(
        &self,
        id: RequestId,
        mut request: Request,
    ) -> Result<RequestId, QueueFull> {
        request.id = id;
        let now = self.shared.transport.rendered();
        match request.when {
            When::AfterSamples(samples) => {
                request.when = When::AtSample(now.saturating_add(samples))
            }
            When::AtTime(at) => {
                if let Some(sample) = self.shared.transport.sample_at(at) {
                    request.when = When::AtSample(sample);
                }
            }
            _ => {}
        }
        if let Some(ttl) = request.ttl.take() {
            request.expires = Some(now.saturating_add(ttl));
        }
        // Counted before it is queued, so the drain never releases a slot
        // not yet taken.
        let watching = matches!(request.when, When::Condition(_));
        if watching {
            self.shared.watches.take();
        }
        // A reserved request may take any slot: the handles that leave
        // slots free leave some for it.
        let reserve = match request.reserved {
            true => 0,
            false => self.reserve,
        };
        let reserved = request.reserved;
        let pushed = if reserve == 0 || self.queue.leaves(reserve) {
            self.queue.try_push(request)
        } else {
            Err(request)
        };
        match pushed {
            Ok(()) => {
                if reserved {
                    self.shared.reserved.queued.fetch_add(1, Ordering::Release);
                }
                Ok(id)
            }
            Err(request) => {
                if watching {
                    self.shared.watches.release();
                }
                self.shared.overflows.record();
                Err(QueueFull(request))
            }
        }
    }

    /// Whether the channel's drain stopped reading: a render panicked, and
    /// every submission is refused (see `graph::unwind`).
    pub(crate) fn closed(&self) -> bool {
        self.queue.is_closed()
    }

    /// Whether a request submitted now would find room. Exact while
    /// submissions are serialized, as a live graph's are (under its
    /// publisher): the audio thread only frees slots.
    pub(crate) fn has_room(&self) -> bool {
        self.queue.leaves(self.reserve)
    }

    /// Whether `count` requests submitted now, one after another, would all
    /// find room. Exact under the same terms as [`Self::has_room`].
    pub(crate) fn has_room_for(&self, count: u64) -> bool {
        let Some(last) = count.checked_sub(1) else {
            return true;
        };
        let ahead = self.reserve.saturating_add(last);
        ahead < self.queue.capacity() && self.queue.leaves(ahead)
    }

    /// Whether [`CONDITION_CAPACITY`](super::CONDITION_CAPACITY) requests
    /// timed by a condition are in flight, so a front door refuses one more
    /// before submitting it. Exact while submissions are serialized, as a
    /// live graph's are (under its publisher): the audio thread only
    /// releases, so a stale count only refuses conservatively.
    pub(crate) fn watchers_full(&self) -> bool {
        self.shared.watches.full()
    }

    /// Notes that timed edit `id`, queued through this channel, was
    /// cancelled (see [`WatchSlots`]).
    pub(crate) fn cancelled(&self, id: RequestId) {
        self.shared.watches.cancel(id);
    }

    /// Samples rendered as of the latest block ([`Transport::rendered`]):
    /// what [`When::AfterSamples`] counts from.
    pub(crate) fn rendered(&self) -> u64 {
        self.shared.transport.rendered()
    }

    /// Where the queue stands: every request submitted so far sits before
    /// it (see [`Self::taken_before`]).
    pub(crate) fn queued(&self) -> u64 {
        self.queue.position()
    }

    /// Whether the drain has taken up every request submitted before
    /// `queued`, a position [`Self::queued`] returned.
    pub(crate) fn taken_before(&self, queued: u64) -> bool {
        self.queue.taken_before(queued)
    }

    /// The count the channel's drain releases requests timed by a
    /// condition from as they settle.
    pub(crate) fn watch_slots(&self) -> Arc<WatchSlots> {
        Arc::clone(&self.shared.watches)
    }

    /// The count of reserved requests queued, for the channel's drain.
    pub(crate) fn reserved_queued(&self) -> Arc<ReservedQueued> {
        Arc::clone(&self.shared.reserved)
    }

    /// Submissions refused for want of room, for telemetry.
    pub(crate) fn overflows(&self) -> &EventCounter {
        &self.shared.overflows
    }
}
