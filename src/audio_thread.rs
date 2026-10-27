//! Marks the audio thread so debug builds catch work that must never run
//! there.
//!
//! `SignalGraph::process_block` runs inside an [`AudioThreadScope`]. Two
//! kinds of check read it, both panicking in debug builds and compiled out
//! of release builds:
//!
//! - dropping a request payload (`crate::payload`), which must be retired
//!   to a control thread instead;
//! - control-only entry points ([`debug_assert_control_thread`]): creating
//!   or freeing a request queue, resolving a request's target under the
//!   publisher lock, or taking a [`ControlLock`]. A module's `process()`
//!   reaching one would risk a dropout.

use std::marker::PhantomData;
use std::sync::{LockResult, Mutex, MutexGuard, TryLockResult};

#[cfg(debug_assertions)]
thread_local! {
    static ON_AUDIO_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the current thread as the audio thread until dropped. Saves and
/// restores the previous state, so scopes nest. A no-op in release builds;
/// never allocates.
#[must_use = "the scope ends when this value is dropped"]
pub(crate) struct AudioThreadScope {
    #[cfg(debug_assertions)]
    outer: bool,
    /// Tied to the thread whose flag it set.
    _not_send: PhantomData<*const ()>,
}

impl AudioThreadScope {
    #[inline]
    pub(crate) fn enter() -> Self {
        Self {
            #[cfg(debug_assertions)]
            outer: ON_AUDIO_THREAD.with(|flag| flag.replace(true)),
            _not_send: PhantomData,
        }
    }
}

impl Drop for AudioThreadScope {
    #[inline]
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        ON_AUDIO_THREAD.with(|flag| flag.set(self.outer));
    }
}

/// Whether the current thread is inside an [`AudioThreadScope`] and not
/// already unwinding. Always false in release builds.
#[inline]
pub(crate) fn on_audio_thread() -> bool {
    #[cfg(debug_assertions)]
    return ON_AUDIO_THREAD.try_with(std::cell::Cell::get) == Ok(true) && !std::thread::panicking();
    #[cfg(not(debug_assertions))]
    false
}

/// Panics in debug builds when control-only code (`what`) runs inside an
/// [`AudioThreadScope`]. Call it first thing in every control-only entry
/// point.
#[inline]
pub(crate) fn debug_assert_control_thread(what: &str) {
    if on_audio_thread() {
        panic!("{what} called on the audio thread: it is control-only");
    }
}

/// A lock only control threads take, around module state the audio thread
/// never reads (a scheduler's written schedule, an agent's prompt). Taking
/// it inside an [`AudioThreadScope`] panics in debug builds, so a module
/// whose `process()` reaches one fails its tests instead of risking a
/// dropout. Module control state never sits behind a bare `std::sync`
/// lock: see `crate::modules::lock_scan_tests`.
#[derive(Default)]
pub(crate) struct ControlLock<T>(Mutex<T>);

impl<T> ControlLock<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    /// Takes the lock, as [`Mutex::lock`] does. Control threads only.
    pub(crate) fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        debug_assert_control_thread("taking a control lock");
        self.0.lock()
    }

    /// Takes the lock if no other thread holds it, as [`Mutex::try_lock`]
    /// does. It never waits, so any thread may call it.
    pub(crate) fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        self.0.try_lock()
    }
}
