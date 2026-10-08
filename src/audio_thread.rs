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
//!   or freeing a request queue, or resolving a request's target under the
//!   publisher lock. A module's `process()` reaching one would risk a
//!   dropout.

use std::marker::PhantomData;

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
