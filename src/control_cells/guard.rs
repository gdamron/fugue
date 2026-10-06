//! The control-side lock for multi-cell edits, and the debug check that the
//! audio thread never takes it.

use std::marker::PhantomData;
use std::sync::PoisonError;

use super::sync::{Mutex, MutexGuard};

/// Serializes multi-cell edits among control threads (R3).
///
/// Never taken on the audio thread: the audio side only stores single cells
/// and reads snapshots, neither of which needs it. Debug builds panic if
/// [`lock`](Self::lock) is called inside `SignalGraph::process_block` (see
/// [`AudioBlockScope`]).
pub(crate) struct ControlLock {
    mutex: Mutex<()>,
}

/// Proof that the holder is a control thread holding a [`ControlLock`].
pub(crate) struct ControlGuard<'a> {
    _held: MutexGuard<'a, ()>,
}

impl ControlLock {
    pub(crate) fn new() -> Self {
        Self {
            mutex: Mutex::new(()),
        }
    }

    /// Blocks until this control thread holds the lock.
    ///
    /// Poisoning is ignored: the lock protects no data of its own, and the
    /// cells it serializes edits to are valid after any panic.
    ///
    /// # Panics
    ///
    /// In debug builds, if called inside `SignalGraph::process_block`.
    pub(crate) fn lock(&self) -> ControlGuard<'_> {
        debug_assert_off_audio_thread("ControlLock::lock");
        ControlGuard {
            _held: self.mutex.lock().unwrap_or_else(PoisonError::into_inner),
        }
    }
}

#[cfg(debug_assertions)]
thread_local! {
    static IN_AUDIO_BLOCK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Marks the current thread as inside `SignalGraph::process_block` until
/// dropped, so debug builds can catch control-side locking on the audio
/// thread. Saves and restores the previous state, so scopes nest. A no-op in
/// release builds; never allocates.
#[must_use = "the scope ends when this value is dropped"]
pub(crate) struct AudioBlockScope {
    #[cfg(debug_assertions)]
    outer: bool,
    /// Tied to the thread whose flag it set.
    _not_send: PhantomData<*const ()>,
}

impl AudioBlockScope {
    #[inline]
    pub(crate) fn enter() -> Self {
        Self {
            #[cfg(debug_assertions)]
            outer: IN_AUDIO_BLOCK.with(|flag| flag.replace(true)),
            _not_send: PhantomData,
        }
    }
}

impl Drop for AudioBlockScope {
    #[inline]
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        IN_AUDIO_BLOCK.with(|flag| flag.set(self.outer));
    }
}

/// Panics in debug builds if the current thread is inside an
/// [`AudioBlockScope`].
#[inline]
pub(super) fn debug_assert_off_audio_thread(what: &str) {
    #[cfg(debug_assertions)]
    assert!(
        !IN_AUDIO_BLOCK.with(std::cell::Cell::get),
        "{what} called inside process_block: the audio thread must never take control-side locks"
    );
    #[cfg(not(debug_assertions))]
    let _ = what;
}
