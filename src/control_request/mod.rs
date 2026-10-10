//! Control requests: how every control write reaches the audio thread.
//!
//! A control write (from RPC, MCP agents, scripts or reload) becomes a
//! typed [`Request`] submitted through a [`RequestSender`] into one
//! bounded, allocation-free MPSC queue ([`bounded`]). The audio thread is
//! the queue's only consumer and the only mutator of module control state,
//! and it reports what became of each request through an outcome channel
//! ([`outcome_channel`]). Modules declare their controls in a
//! [`ControlTable`], which control threads resolve keys and coerce values
//! against, apply them with [`Module::apply`](crate::Module::apply) and
//! publish what they hold to [`ControlCells`] for read-back. Requests are
//! timed on the engine's sample transport ([`Transport`]): now, at a
//! sample, or some samples from submission, with an optional ttl.
//!
//! Every ordering argument lives in [`queue`](self::queue) and [`EventCounter`],
//! and the `loom_tests` models check the shipped source files against loom's
//! atomics and cells (the `sync` shim below), so modules never need one.
//!
//! The drain that feeds the pending store lives with the graph, in
//! `invention::graph::requests`. Front doors and the module control tables
//! that use the rest come with FUG-310 and FUG-317, hence the `dead_code`
//! and `unused_imports` allowance below; remove it once they land.
#![allow(dead_code, unused_imports)]

mod automation;
mod cells;
mod declare;
mod event;
mod key;
mod legacy;
mod local;
mod outcome;
mod pending;
mod queue;
mod request;
mod sender;
mod transport;

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

pub(crate) use automation::{take_automation, Automation};
pub(crate) use cells::{apply_declared, ControlCells};
pub(crate) use declare::{integer_domain, ControlDecl, ControlTable, DeclKind, Writer};
pub(crate) use event::{EventCounter, EventCursor};
pub(crate) use legacy::LEGACY_CONTROLS;
pub(crate) use local::{local_controls, local_get, local_set};
pub(crate) use key::{ControlKey, ControlKeys, RtScalar};
pub(crate) use outcome::{outcome_channel, OutcomeReceiver, OutcomeSender};
pub(crate) use pending::{expired, Outcome, Outcomes, PendingStore, Refusal};
pub(crate) use queue::{bounded, QueueConsumer, QueueProducer};
pub(crate) use request::{
    ControlIndex, ControlTarget, Intent, Request, RequestId, RequestValue, RtValue, Source, When,
};
pub(crate) use sender::{request_channel, QueueFull, RequestSender};
pub(crate) use transport::Transport;

#[cfg(test)]
mod loom_tests;
#[cfg(test)]
mod tests;
