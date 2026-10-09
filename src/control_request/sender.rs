//! Submitting requests to the audio thread.

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
    });
    (RequestSender { queue, shared }, consumer)
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
}

struct SenderShared {
    /// The next request id. `Relaxed` is enough: the counter only has to
    /// hand out distinct values, which RMW atomicity guarantees.
    next_id: AtomicU64,
    overflows: EventCounter,
    transport: Arc<Transport>,
}

impl RequestSender {
    /// Assigns `request` the channel's next id, resolves its relative times
    /// and queues it. Lock-free and allocation-free from any thread,
    /// including the audio thread.
    ///
    /// [`When::AfterSamples`] and the `ttl` count from the transport's
    /// published count ([`Transport::rendered`]) as of this call: they
    /// become [`When::AtSample`] and the request's `expires` sample. They
    /// are resolved once, so a request handed back in [`QueueFull`] keeps
    /// its times when submitted again.
    ///
    /// # Errors
    ///
    /// [`QueueFull`] with the request (id assigned) when the queue has no
    /// room; the overflow is also counted in [`overflows`](Self::overflows).
    pub(crate) fn submit(&self, mut request: Request) -> Result<RequestId, QueueFull> {
        let id = RequestId(self.shared.next_id.fetch_add(1, Ordering::Relaxed));
        request.id = id;
        let now = self.shared.transport.rendered();
        if let When::AfterSamples(samples) = request.when {
            request.when = When::AtSample(now.saturating_add(samples));
        }
        if let Some(ttl) = request.ttl.take() {
            request.expires = Some(now.saturating_add(ttl));
        }
        match self.queue.try_push(request) {
            Ok(()) => Ok(id),
            Err(request) => {
                self.shared.overflows.record();
                Err(QueueFull(request))
            }
        }
    }

    /// Submissions refused for want of room, for telemetry.
    pub(crate) fn overflows(&self) -> &EventCounter {
        &self.shared.overflows
    }
}
