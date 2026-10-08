//! Submitting requests to the audio thread.

use super::event::EventCounter;
use super::queue::{bounded, QueueConsumer, QueueProducer};
use super::request::{Request, RequestId};
use super::sync::{Arc, AtomicU64, Ordering};

/// Creates a request channel holding up to `capacity` requests (rounded as
/// [`bounded`] rounds it). The consumer goes to the audio thread's drain.
/// Allocates once: call it on a control thread.
pub(crate) fn request_channel(capacity: usize) -> (RequestSender, QueueConsumer<Request>) {
    let (queue, consumer) = bounded(capacity);
    let shared = Arc::new(SenderShared {
        next_id: AtomicU64::new(1),
        overflows: EventCounter::new(),
    });
    (RequestSender { queue, shared }, consumer)
}

/// The queue had no room: the request comes back unsent, so a payload it
/// carries is never dropped by the queue.
#[derive(Debug, PartialEq)]
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
}

impl RequestSender {
    /// Assigns `request` the channel's next id and queues it. Lock-free and
    /// allocation-free from any thread, including the audio thread.
    ///
    /// # Errors
    ///
    /// [`QueueFull`] with the request (id assigned) when the queue has no
    /// room; the overflow is also counted in [`overflows`](Self::overflows).
    pub(crate) fn submit(&self, mut request: Request) -> Result<RequestId, QueueFull> {
        let id = RequestId(self.shared.next_id.fetch_add(1, Ordering::Relaxed));
        request.id = id;
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
