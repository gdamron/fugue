//! Per-thread allocator accounting for tests that prove an audio-thread path
//! never allocates or frees.
//!
//! Installed as the global allocator for the library's unit tests only. It
//! forwards to the system allocator and, while a thread is inside
//! [`allocator_events`], counts that thread's allocations and frees; inside
//! [`foreign_frees`], the frees of memory allocated before it began.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct CountingAllocator;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    /// Addresses allocated inside [`foreign_frees`], up to [`OWN_CAPACITY`].
    static OWN: [Cell<usize>; OWN_CAPACITY] = const { [const { Cell::new(0) }; OWN_CAPACITY] };
    static OWN_COUNT: Cell<usize> = const { Cell::new(0) };
    static FOREIGN: Cell<usize> = const { Cell::new(0) };
}

/// Allocations [`foreign_frees`] can tell apart.
const OWN_CAPACITY: usize = 64;

fn note(counter: &'static std::thread::LocalKey<Cell<usize>>) {
    if TRACKING.try_with(Cell::get).unwrap_or(false) {
        let _ = counter.try_with(|count| count.set(count.get() + 1));
    }
}

fn watching() -> bool {
    WATCHING.try_with(Cell::get).unwrap_or(false)
}

/// Remembers `ptr` as allocated inside [`foreign_frees`].
fn note_own(ptr: *mut u8) {
    if !watching() {
        return;
    }
    let _ = OWN_COUNT.try_with(|count| {
        let _ = OWN.try_with(|own| {
            if let Some(slot) = own.get(count.get()) {
                slot.set(ptr as usize);
            }
        });
        count.set(count.get() + 1);
    });
}

/// Counts freeing `ptr` when it was not allocated inside [`foreign_frees`].
fn note_free(ptr: *mut u8) {
    if !watching() {
        return;
    }
    let count = OWN_COUNT.try_with(Cell::get).unwrap_or(0).min(OWN_CAPACITY);
    let own = OWN
        .try_with(|own| own[..count].iter().any(|slot| slot.get() == ptr as usize))
        .unwrap_or(false);
    if !own {
        let _ = FOREIGN.try_with(|foreign| foreign.set(foreign.get() + 1));
    }
}

// SAFETY: every method forwards to the system allocator unchanged; the
// bookkeeping touches only const-initialized thread-locals, which never
// allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(&ALLOCS);
        let ptr = unsafe { System.alloc(layout) };
        note_own(ptr);
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(&ALLOCS);
        let ptr = unsafe { System.alloc_zeroed(layout) };
        note_own(ptr);
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(&ALLOCS);
        note_free(ptr);
        let ptr = unsafe { System.realloc(ptr, layout, new_size) };
        note_own(ptr);
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(&FREES);
        note_free(ptr);
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

/// Runs `f` and returns its result with the number of frees it made on the
/// calling thread of memory allocated before it began (or on another
/// thread). What `f` allocates and frees itself, such as a panic's own
/// bookkeeping as it unwinds, is not counted.
///
/// # Panics
///
/// If `f` allocates more than it can tell apart (64 allocations).
pub(crate) fn foreign_frees<R>(f: impl FnOnce() -> R) -> (R, usize) {
    OWN_COUNT.with(|count| count.set(0));
    FOREIGN.with(|foreign| foreign.set(0));
    WATCHING.with(|watching| watching.set(true));
    let result = f();
    WATCHING.with(|watching| watching.set(false));
    let own = OWN_COUNT.with(Cell::get);
    assert!(
        own <= OWN_CAPACITY,
        "{own} allocations: too many to tell apart"
    );
    (result, FOREIGN.with(Cell::get))
}

#[test]
fn counts_only_the_tracked_thread() {
    let (_, allocs, frees) = allocator_events(|| drop(vec![1u8; 16]));
    assert_eq!((allocs, frees), (1, 1));
    let (_, allocs, frees) = allocator_events(|| 1 + 1);
    assert_eq!((allocs, frees), (0, 0));
}

#[test]
fn counts_only_frees_of_memory_allocated_before() {
    let before = vec![1u8; 16];
    let (_, foreign) = foreign_frees(|| drop(vec![1u8; 16]));
    assert_eq!(foreign, 0);
    let (_, foreign) = foreign_frees(|| drop(before));
    assert_eq!(foreign, 1);
}
