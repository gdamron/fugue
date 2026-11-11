//! The time source that paces video frames.
//!
//! Video workers read the time and wait through a [`VideoClock`] rather than
//! calling `Instant::now()` or sleeping directly, so tests can swap in a
//! clock they step by hand. Every wait is a condvar wait under the worker's
//! own lock, so control changes (stop, pause, restart) end it early.

use std::sync::{Condvar, MutexGuard, Weak};
use std::time::{Duration, Instant};

pub(crate) trait VideoClock: Send + Sync {
    fn now(&self) -> Instant;

    /// How long a worker may block on its condvar while waiting for
    /// `deadline` (`None`: until notified). The worker calls this with its
    /// lock held, right before it waits, and rechecks its state on waking.
    fn wait_limit(&self, deadline: Option<Instant>) -> Option<Duration>;

    /// Registers a worker to wake when this clock's time moves without it
    /// waiting out a timeout. The system clock never does, so it drops these.
    fn on_advance(&self, waiter: Weak<dyn ClockWaiter>);
}

/// A worker woken when a clock's time moves.
pub(crate) trait ClockWaiter: Send + Sync {
    /// Notifies the worker's condvar, under its lock so the wake is not lost.
    // Only a test clock moves on its own; the system clock never calls this.
    #[cfg_attr(not(test), allow(dead_code))]
    fn clock_advanced(&self);
}

/// The real monotonic clock.
pub(crate) struct SystemClock;

impl VideoClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wait_limit(&self, deadline: Option<Instant>) -> Option<Duration> {
        deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    fn on_advance(&self, _waiter: Weak<dyn ClockWaiter>) {}
}

/// Waits on `wake` until it is notified or, on `clock`, `deadline` passes.
/// Like any condvar wait it may return early: callers recheck their state.
pub(super) fn wait_on<'a, T>(
    clock: &dyn VideoClock,
    wake: &Condvar,
    guard: MutexGuard<'a, T>,
    deadline: Option<Instant>,
) -> MutexGuard<'a, T> {
    match clock.wait_limit(deadline) {
        Some(limit) => wake.wait_timeout(guard, limit).unwrap().0,
        None => wake.wait(guard).unwrap(),
    }
}
