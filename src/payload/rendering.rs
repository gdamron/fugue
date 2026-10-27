//! Whether a render is unwinding, in every build.
//!
//! A module that panics with a payload it was handed (in `apply_payload`,
//! or a handover) drops it as the unwind leaves the module, before the
//! graph can catch anything (see `SignalGraph::restoring`). Freeing it
//! there would free on the rendering thread, and run whatever its drop
//! runs, maybe under the publisher (a settle). So a [`Payload`] or
//! [`Shared`] dropped while a render unwinds is leaked instead: once per
//! panic, after which the graph has failed and renders nothing.
//!
//! [`Payload`]: super::Payload
//! [`Shared`]: super::Shared

use std::cell::Cell;
use std::marker::PhantomData;

thread_local! {
    // Const-initialised and without drop glue: reading or writing it never
    // allocates, and it registers no thread-exit destructor.
    static RENDERING: Cell<bool> = const { Cell::new(false) };
}

/// Marks the current thread as rendering a block until dropped
/// (`SignalGraph::process_block` enters one). Saves and restores the
/// previous state, so scopes nest. Allocation-, free- and lock-free.
#[must_use = "the scope ends when this value is dropped"]
pub(crate) struct RenderScope {
    outer: bool,
    /// Tied to the thread whose flag it set.
    _not_send: PhantomData<*const ()>,
}

impl RenderScope {
    #[inline]
    pub(crate) fn enter() -> Self {
        Self {
            outer: RENDERING.with(|flag| flag.replace(true)),
            _not_send: PhantomData,
        }
    }
}

impl Drop for RenderScope {
    #[inline]
    fn drop(&mut self) {
        RENDERING.with(|flag| flag.set(self.outer));
    }
}

/// Whether the current thread is unwinding out of a render: what it drops
/// now must be leaked, not freed.
#[inline]
pub(super) fn unwinding_a_render() -> bool {
    std::thread::panicking() && RENDERING.try_with(Cell::get) == Ok(true)
}
