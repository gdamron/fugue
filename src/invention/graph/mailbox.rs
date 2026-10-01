//! A single-slot, lock-free handoff of boxed values between threads.

use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

/// Holds at most one boxed value for another thread to take.
///
/// Putting and taking are single atomic swaps: neither side ever blocks,
/// locks, allocates, or frees. Ownership of the box moves wholesale with the
/// pointer, so a value is owned by exactly one side at a time. The putter can
/// take an untaken value back, which lets the control thread fold a
/// publication the audio thread has not yet seen into the next one instead of
/// queueing behind a stalled stream.
pub(crate) struct Mailbox<T> {
    slot: AtomicPtr<T>,
}

// SAFETY: the mailbox only ever transfers ownership of a `Box<T>` between
// threads, so it is as thread-safe as sending `T` itself.
unsafe impl<T: Send> Send for Mailbox<T> {}
// SAFETY: every access to the slot is an atomic swap that moves ownership;
// no `&T` is ever shared through it.
unsafe impl<T: Send> Sync for Mailbox<T> {}

impl<T> Mailbox<T> {
    /// Creates an empty mailbox.
    pub(crate) fn new() -> Self {
        Self {
            slot: AtomicPtr::new(ptr::null_mut()),
        }
    }

    /// Puts `value` in the slot, returning the value it displaced, if any.
    pub(crate) fn put(&self, value: Box<T>) -> Option<Box<T>> {
        let previous = self.slot.swap(Box::into_raw(value), Ordering::AcqRel);
        // SAFETY: a non-null pointer in the slot always came from
        // `Box::into_raw` and has not been reclaimed: the swap transferred
        // sole ownership of it to us.
        (!previous.is_null()).then(|| unsafe { Box::from_raw(previous) })
    }

    /// Takes the value out of the slot, if any. Allocation- and lock-free.
    pub(crate) fn take(&self) -> Option<Box<T>> {
        let taken = self.slot.swap(ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: as in `put`, the swap transferred sole ownership.
        (!taken.is_null()).then(|| unsafe { Box::from_raw(taken) })
    }
}

impl<T> Drop for Mailbox<T> {
    fn drop(&mut self) {
        drop(self.take());
    }
}

#[cfg(test)]
mod tests {
    use super::Mailbox;

    #[test]
    fn put_displaces_and_take_empties() {
        let mailbox = Mailbox::new();
        assert!(mailbox.take().is_none());
        assert!(mailbox.put(Box::new(1)).is_none());
        assert_eq!(mailbox.put(Box::new(2)).as_deref(), Some(&1));
        assert_eq!(mailbox.take().as_deref(), Some(&2));
        assert!(mailbox.take().is_none());
    }
}
