//! Control requests: how every control write reaches the audio thread.
//!
//! A control write (from RPC, MCP agents, scripts, reload, or the audio
//! thread's own automation) becomes a typed [`Request`] submitted through a
//! [`RequestSender`] into one bounded, allocation-free MPSC queue
//! ([`bounded`]). The audio thread is the queue's only consumer and the only
//! mutator of module control state.
//!
//! Every ordering argument lives in [`queue`](self::queue) and [`EventCounter`],
//! and the `loom_tests` models check the shipped source files against loom's
//! atomics and cells (the `sync` shim below), so modules never need one.
//!
//! This module holds the request types, the queue, the sender and the
//! audio side's pending store. The drain that feeds the store (with the
//! graph, in `invention::graph::requests`), the outcomes path, typed control
//! keys and the module control tables come in later slices (FUG-308, FUG-310),
//! hence the `dead_code` and `unused_imports` allowance below; remove it
//! once consumers land.
#![allow(dead_code, unused_imports)]

mod event;
mod pending;
mod queue;
mod request;
mod sender;

/// The atomics and cell the queue is built on. The loom models compile the
/// same source files against loom's versions instead.
mod sync {
    pub(super) use std::hint::spin_loop;
    pub(super) use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    pub(super) use std::sync::Arc;

    pub(super) use crate::audio_thread::debug_assert_control_thread;

    /// `std`'s `UnsafeCell` behind `loom::cell::UnsafeCell`'s closure API,
    /// so the queue's slot accesses compile unchanged against loom, which
    /// checks every one of them for data races.
    pub(super) struct UnsafeCell<T>(std::cell::UnsafeCell<T>);

    impl<T> UnsafeCell<T> {
        pub(super) fn new(value: T) -> Self {
            Self(std::cell::UnsafeCell::new(value))
        }

        #[inline]
        pub(super) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
            f(self.0.get())
        }

        #[inline]
        pub(super) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
            f(self.0.get())
        }
    }
}

pub(crate) use event::{EventCounter, EventCursor};
pub(crate) use pending::{Outcome, Outcomes, PendingStore, Refusal};
pub(crate) use queue::{bounded, QueueConsumer, QueueProducer};
pub(crate) use request::{
    ControlIndex, ControlTarget, Intent, Request, RequestId, RequestValue, RtValue, Source, When,
};
pub(crate) use sender::{request_channel, QueueFull, RequestSender};

#[cfg(test)]
mod loom_tests;
#[cfg(test)]
mod tests;
