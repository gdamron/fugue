//! Per-thread allocator accounting for tests that prove an audio-thread path
//! never allocates or frees.
//!
//! Installed as the global allocator for the library's unit tests only. It
//! forwards to the system allocator and, while a thread is inside
//! [`allocator_events`], counts that thread's allocations and frees.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}

fn note(counter: &'static std::thread::LocalKey<Cell<usize>>) {
    if TRACKING.try_with(Cell::get).unwrap_or(false) {
        let _ = counter.try_with(|count| count.set(count.get() + 1));
    }
}

// SAFETY: every method forwards to the system allocator unchanged; the
// bookkeeping touches only const-initialized thread-locals, which never
// allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(&ALLOCS);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(&ALLOCS);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(&ALLOCS);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(&FREES);
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Runs `f` and returns its result with the number of allocations
/// (including reallocations) and frees it made on the calling thread.
pub(crate) fn allocator_events<R>(f: impl FnOnce() -> R) -> (R, usize, usize) {
    ALLOCS.with(|count| count.set(0));
    FREES.with(|count| count.set(0));
    TRACKING.with(|tracking| tracking.set(true));
    let result = f();
    TRACKING.with(|tracking| tracking.set(false));
    (result, ALLOCS.with(Cell::get), FREES.with(Cell::get))
}

#[test]
fn counts_only_the_tracked_thread() {
    let (_, allocs, frees) = allocator_events(|| drop(vec![1u8; 16]));
    assert_eq!((allocs, frees), (1, 1));
    let (_, allocs, frees) = allocator_events(|| 1 + 1);
    assert_eq!((allocs, frees), (0, 0));
}
