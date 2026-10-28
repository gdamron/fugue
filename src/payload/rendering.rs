//! Whether a render is unwinding, in every build, and what it drops then.
//!
//! A module that panics with a payload it was handed (in `apply_payload`,
//! or a handover) drops it as the unwind leaves the module, before the
//! graph can catch anything (see `SignalGraph::restoring`). Freeing it
//! there would free on the rendering thread, and run whatever its drop
//! runs, maybe under the publisher (a settle). So a [`Payload`] or
//! [`Shared`] dropped while a render unwinds is caught instead: its
//! pointer is set aside on the thread, and the graph takes it as the panic
//! leaves the block ([`take_caught`]), keeping it for its teardown on a
//! control thread. A value a module unwraps from a payload and holds while
//! it calls into others (a development's inner modules) is kept in a
//! [`RetireOnUnwind`] for the same reason. Past [`CAUGHT`] of them in one
//! unwind, or caught where no graph takes them, they are leaked.
//!
//! [`Payload`]: super::Payload
//! [`Shared`]: super::Shared

use std::any::Any;
use std::cell::Cell;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::sync::Arc;

use super::Retired;

/// Values one unwind can set aside for the graph to keep.
pub(crate) const CAUGHT: usize = 4;

/// A [`Retired`] value's pointer, set aside while a render unwinds.
#[derive(Clone, Copy)]
enum Caught {
    Owned(*mut (dyn Any + Send)),
    Shared(*const (dyn Any + Send + Sync)),
}

thread_local! {
    // Const-initialised and without drop glue (raw pointers): reading or
    // writing them never allocates, and they register no thread-exit
    // destructor.
    static RENDERING: Cell<bool> = const { Cell::new(false) };
    static CAUGHT_ON_UNWIND: Cell<[Option<Caught>; CAUGHT]> = const { Cell::new([None; CAUGHT]) };
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
/// now must be caught ([`catch`]), not freed.
#[inline]
pub(super) fn unwinding_a_render() -> bool {
    std::thread::panicking() && RENDERING.try_with(Cell::get) == Ok(true)
}

/// Sets `value`, dropped while a render unwinds, aside for the graph to
/// take ([`take_caught`]), or leaks it when [`CAUGHT`] are set aside
/// already. Allocation-, free- and lock-free.
pub(super) fn catch(value: Retired) {
    let value = ManuallyDrop::new(value);
    // SAFETY: the pointer is read out once and `value` is never dropped,
    // so it is owned by the slot (or leaked) from here.
    let caught = unsafe {
        match &*value {
            Retired::Owned(owned) => Caught::Owned(Box::into_raw(ptr::read(owned))),
            Retired::Shared(shared) => Caught::Shared(Arc::into_raw(ptr::read(shared))),
        }
    };
    let _ = CAUGHT_ON_UNWIND.try_with(|slots| {
        let mut held = slots.get();
        if let Some(slot) = held.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(caught);
            slots.set(held);
        }
    });
}

/// Every value an unwind on this thread set aside ([`catch`]), for the
/// graph to keep until it is torn down off the audio thread. Call it once
/// the unwind is caught. Allocation-, free- and lock-free.
pub(crate) fn take_caught() -> [Option<Retired>; CAUGHT] {
    let held = CAUGHT_ON_UNWIND
        .try_with(|slots| slots.replace([None; CAUGHT]))
        .unwrap_or([None; CAUGHT]);
    // SAFETY: each pointer came from `into_raw` in `catch` and was taken
    // out of its slot just now, so it is turned back into its owner once.
    held.map(|caught| {
        caught.map(|caught| unsafe {
            match caught {
                Caught::Owned(owned) => Retired::Owned(Box::from_raw(owned)),
                Caught::Shared(shared) => Retired::Shared(Arc::from_raw(shared)),
            }
        })
    })
}

/// A value the audio side holds while it calls into a module that may
/// panic: dropped as usual, but set aside for the graph if dropped while a
/// render unwinds ([`catch`]).
pub(crate) struct RetireOnUnwind<T: Into<Retired>>(ManuallyDrop<T>);

impl<T: Into<Retired>> RetireOnUnwind<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(ManuallyDrop::new(value))
    }

    /// The value, out of the guard.
    pub(crate) fn into_inner(self) -> T {
        let mut this = ManuallyDrop::new(self);
        // SAFETY: `this` is never dropped, so the value is taken once.
        unsafe { ManuallyDrop::take(&mut this.0) }
    }
}

impl<T: Into<Retired>> Deref for RetireOnUnwind<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Into<Retired>> DerefMut for RetireOnUnwind<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: Into<Retired>> Drop for RetireOnUnwind<T> {
    fn drop(&mut self) {
        if unwinding_a_render() {
            // SAFETY: taken once, here; never used again.
            return catch(unsafe { ManuallyDrop::take(&mut self.0) }.into());
        }
        // SAFETY: dropped once, here; never used again.
        unsafe { ManuallyDrop::drop(&mut self.0) }
    }
}
