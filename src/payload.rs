//! Heavy request payloads: built on a control thread, owned by the audio
//! side once applied, and freed on a control thread.
//!
//! A schedule, pattern, sequence bank, sample or prepared edit is too big to
//! copy into a request, so a request carries it as a [`Payload`], an erased
//! shared pointer. The module that applies it keeps it as a [`Shared<T>`].
//!
//! # Ownership
//!
//! 1. A control thread builds the value whole and wraps it with
//!    [`Payload::new`] (or converts a [`Shared<T>`] it holds).
//! 2. The payload is moved, never cloned, into a request. The request
//!    queue's release/acquire handoff publishes the finished value to the
//!    audio thread.
//! 3. On the audio side every `Payload` leaves by exactly one of two
//!    routes: [`Payload::downcast`] into the `Shared<T>` a module keeps, or
//!    [`Retirer::retire`] (superseded, wrong type, target gone, ...). The
//!    value a module replaces is retired too, and so is every audio-side
//!    clone of a `Shared<T>`. No accessor hands out the raw [`Arc`], so a
//!    clone cannot escape this rule.
//! 4. A [`Retirer`], the one producer of its [`RetireQueue`], pushes
//!    retirements onto that bounded single-producer, single-consumer ring
//!    (`crate::spsc`). A push never waits for the consumer: it loads
//!    `head` (Acquire, pairing with the Release that frees a slot once its
//!    value is read) and either writes a free slot and publishes it with a
//!    Release store of `tail`, or hands the value back at once. A slot the
//!    consumer is still reading counts as full. Pushing allocates, frees
//!    and locks nothing.
//! 5. A control thread drains the queue: under the queue's consumer lock
//!    it loads `tail` (Acquire, pairing with the push's Release), moves the
//!    value out and releases the slot, then drops the value after the
//!    lock. The graph `Reclaimer` owns one queue per engine and drains it.
//!
//! So whichever thread drops the last `Arc` of a payload is a control
//! thread. Modules, and the `Shared<T>`s they hold, are torn down off the
//! audio thread, since graph publication retires them to the reclaimer.
//! Release builds rely on this discipline; debug builds enforce it: inside
//! an [`AudioThreadScope`](crate::audio_thread::AudioThreadScope) (which
//! `SignalGraph::process_block` enters), dropping a `Payload`, `Shared<T>`
//! or [`Retired`] panics.
//!
//! # Backpressure: defer at apply
//!
//! When the reclaimer stalls, the retire queue fills and the `Retirer`
//! holds up to [`RETIRE_HOLD`] retirements in a preallocated buffer. The
//! request drain must never retire past that, so it calls
//! [`Retirer::flush`] at the start of each block, and takes a payload
//! request only while [`Retirer::has_room`] covers
//! [`MAX_RETIRES_PER_REQUEST`] for it on top of what every payload request
//! still waiting for its sample has reserved (see
//! `control_request::Outcomes`). When there is no room it stops draining
//! for the block: requests
//! stay queued in order, the next block retries, and a request queue that
//! fills up meanwhile reaches submitters as `QueueFull`. Refusing at apply
//! cannot work (the refused payload itself needs a retirement) and refusing
//! at submit would need a saturation signal that is stale by the time a
//! submitter reads it.

// The request drain (FUG-308) retires payloads; no module keeps one until
// FUG-310 and its siblings migrate the first modules.
#![cfg_attr(not(test), allow(dead_code))]

use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use crate::audio_thread::on_audio_thread;
use crate::spsc::{Producer, Ring};

/// Retirements an engine's [`RetireQueue`] holds before its [`Retirer`]
/// starts holding them. The reclaimer drains every few tens of ms.
pub(crate) const RETIRE_CAPACITY: usize = 256;

/// Retirements a [`Retirer`] can hold while the queue is full.
pub(crate) const RETIRE_HOLD: usize = 64;

/// The most retirements applying one request may make: the value a module
/// replaces plus one superseded or refused payload. The drain reserves this
/// much room before taking each request.
pub(crate) const MAX_RETIRES_PER_REQUEST: usize = 2;

/// The erased, single-owner handle a request carries. Not `Clone`: it is
/// moved through the request queue and leaves the audio side by
/// [`Self::downcast`] or [`Retirer::retire`].
pub(crate) struct Payload(Option<Arc<dyn Any + Send + Sync>>);

impl Payload {
    /// Wraps `value`. Control thread only: allocates.
    pub(crate) fn new<T: Send + Sync + 'static>(value: T) -> Self {
        Self(Some(Arc::new(value)))
    }

    /// Whether the payload holds a `T`.
    pub(crate) fn is<T: 'static>(&self) -> bool {
        self.0.as_deref().is_some_and(|value| value.is::<T>())
    }

    /// The payload as the typed handle a module keeps, or the payload back
    /// when it holds another type. Allocation- and free-free.
    pub(crate) fn downcast<T: Send + Sync + 'static>(mut self) -> Result<Shared<T>, Payload> {
        match take(&mut self.0).downcast::<T>() {
            Ok(value) => Ok(Shared(Some(value))),
            Err(value) => Err(Self(Some(value))),
        }
    }
}

impl<T: Send + Sync + 'static> From<Shared<T>> for Payload {
    /// Sends a value the control side keeps a handle on.
    fn from(mut shared: Shared<T>) -> Self {
        let value = take(&mut shared.0);
        Self(Some(value))
    }
}

impl fmt::Debug for Payload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Payload(..)")
    }
}

impl Drop for Payload {
    fn drop(&mut self) {
        if self.0.is_some() {
            debug_assert_off_audio_thread("a Payload");
        }
    }
}

/// The typed handle a module keeps. `Clone` is a reference-count increment;
/// an audio-side clone must be retired, not dropped.
pub(crate) struct Shared<T>(Option<Arc<T>>);

impl<T: Send + Sync + 'static> Shared<T> {
    /// A module's initial value. Control thread only: allocates.
    pub(crate) fn new(value: T) -> Self {
        Self(Some(Arc::new(value)))
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0.as_deref().expect(EMPTY)
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        if self.0.is_some() {
            debug_assert_off_audio_thread("a Shared value");
        }
    }
}

/// A value the audio side has finished with, type-erased so one queue
/// serves every type. Every conversion into it is an unsizing coercion of a
/// pointer the audio side already holds, so building one never allocates.
// The pointers are never read, only held until a control thread drops them.
#[allow(dead_code)]
pub(crate) enum Retired {
    /// A box the audio side owned outright.
    Owned(Box<dyn Any + Send>),
    /// A shared value: a payload, or a module's replaced or cloned handle.
    Shared(Arc<dyn Any + Send + Sync>),
}

impl<T: Send + 'static> From<Box<T>> for Retired {
    fn from(value: Box<T>) -> Self {
        Self::Owned(value)
    }
}

impl<T: Send + Sync + 'static> From<Arc<T>> for Retired {
    fn from(value: Arc<T>) -> Self {
        Self::Shared(value)
    }
}

impl From<Payload> for Retired {
    fn from(mut payload: Payload) -> Self {
        Self::Shared(take(&mut payload.0))
    }
}

impl<T: Send + Sync + 'static> From<Shared<T>> for Retired {
    fn from(mut shared: Shared<T>) -> Self {
        let value = take(&mut shared.0);
        Self::Shared(value)
    }
}

const EMPTY: &str = "a payload handle is only empty while it is converted";

/// Moves the pointer out of a handle being converted, so its `Drop` sees
/// nothing to check.
fn take<P>(slot: &mut Option<P>) -> P {
    slot.take().expect(EMPTY)
}

impl Drop for Retired {
    fn drop(&mut self) {
        debug_assert_off_audio_thread("a Retired value");
    }
}

/// A bounded queue of retired values: one [`Retirer`] pushes, a control
/// thread drains (`with_capacity`, `drain`) and drops what it pops.
pub(crate) type RetireQueue = Ring<Retired>;

/// The audio side's end of a [`RetireQueue`], one per request drain and the
/// queue's only producer. Built on a control thread, then owned by the
/// audio side.
pub(crate) struct Retirer {
    producer: Producer<Retired>,
    /// Retirements the queue had no room for, oldest first. Preallocated;
    /// never grows past `limit`.
    hold: VecDeque<Retired>,
    limit: usize,
    leaked: usize,
}

impl Retirer {
    /// A retirer that can hold `hold` retirements while `queue` is full.
    /// Control thread only: allocates the hold buffer.
    ///
    /// # Panics
    ///
    /// If `queue` already has a live `Retirer`.
    pub(crate) fn new(queue: Arc<RetireQueue>, hold: usize) -> Self {
        Self {
            producer: Producer::claim(queue),
            hold: VecDeque::with_capacity(hold),
            limit: hold,
            leaked: 0,
        }
    }

    /// Audio side: retries held retirements, oldest first, stopping at the
    /// first the queue has no room for. Allocation-, free- and lock-free.
    pub(crate) fn flush(&mut self) {
        while let Some(retired) = self.hold.pop_front() {
            if let Err(retired) = self.producer.push(retired) {
                self.hold.push_front(retired);
                return;
            }
        }
    }

    /// Whether `n` more retirements are sure to fit. Counts hold vacancy
    /// only, ignoring free queue slots, so it is conservative.
    pub(crate) fn has_room(&self, n: usize) -> bool {
        self.limit - self.hold.len() >= n
    }

    /// Audio side: hands `value` to a control thread, or holds it while the
    /// queue is full. Never drops it, and allocation-, free- and lock-free.
    ///
    /// Retiring with both the queue and the hold full is a contract
    /// violation: the caller skipped [`Self::has_room`]. Debug builds panic;
    /// release builds leak the value (see [`Self::leaked`]) rather than free
    /// it on the audio thread.
    pub(crate) fn retire(&mut self, value: impl Into<Retired>) {
        let Err(retired) = self.producer.push(value.into()) else {
            return;
        };
        if self.hold.len() < self.limit {
            self.hold.push_back(retired);
            return;
        }
        std::mem::forget(retired);
        self.leaked += 1;
        if cfg!(debug_assertions) {
            panic!("retired past a full Retirer: check has_room first");
        }
    }

    /// Retirements waiting for room in the queue.
    pub(crate) fn held(&self) -> usize {
        self.hold.len()
    }

    /// Retirements leaked by contract violations (release builds only).
    pub(crate) fn leaked(&self) -> usize {
        self.leaked
    }
}

impl Drop for Retirer {
    /// Sends held retirements on; any the queue has no room for drop here.
    /// A retirer is torn down off the audio thread, like the engine that
    /// owns it. Its claim on the queue is released after this final flush.
    fn drop(&mut self) {
        self.flush();
    }
}

/// Panics in debug builds when `what` is dropped inside an
/// [`AudioThreadScope`](crate::audio_thread::AudioThreadScope), unless the
/// thread is already unwinding.
#[inline]
fn debug_assert_off_audio_thread(what: &str) {
    if on_audio_thread() {
        panic!("{what} was dropped on the audio thread: retire it instead");
    }
}

#[cfg(test)]
mod tests;
