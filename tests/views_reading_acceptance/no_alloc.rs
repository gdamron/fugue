//! `take_view_reading` performs no heap allocation and no free on
//! the calling thread, with a clock that exists, one that does not, and None.

use super::support::*;
use fugue::ViewReading;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

// Counted per thread, and only while a measurement is open, so concurrently
// running tests and the test harness do not see each other.
thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

// `alloc_zeroed` and `realloc` default to calling `alloc`, so they count.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if MEASURING.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if MEASURING.try_with(Cell::get).unwrap_or(false) {
            let _ = FREES.try_with(|c| c.set(c.get() + 1));
        }
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

/// Runs `f`, returning its result with the allocations and frees it made on
/// this thread.
fn measured<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    ALLOCS.with(|c| c.set(0));
    FREES.with(|c| c.set(0));
    MEASURING.with(|m| m.set(true));
    let out = f();
    MEASURING.with(|m| m.set(false));
    (out, ALLOCS.with(Cell::get), FREES.with(Cell::get))
}

#[test]
fn reading_views_does_not_allocate_or_free() {
    let mut engine = engine(12.0);
    for clock in [Some(CLOCK), Some("no_such_clock"), None] {
        // Repeat, as a player does many times a second, with output between.
        for round in 0..20 {
            render(&mut engine, 480 + round * 17, 128);
            let (reading, allocs, frees) = measured(|| engine.take_view_reading(clock));
            assert_eq!(allocs, 0, "{clock:?} round {round} allocated");
            assert_eq!(frees, 0, "{clock:?} round {round} freed");
            let reading: ViewReading = reading.expect("a piece is loaded");
            assert!(reading.levels.frames > 0);
        }
    }
}

#[test]
fn reading_views_without_rendering_does_not_allocate_or_free() {
    // Back to back reads (an empty stretch) take the same path.
    let mut engine = engine(-9.0);
    render(&mut engine, 4_800, 256);
    for clock in [Some(CLOCK), Some("no_such_clock"), None, Some(CLOCK)] {
        let (reading, allocs, frees) = measured(|| engine.take_view_reading(clock));
        assert_eq!((allocs, frees), (0, 0), "{clock:?}");
        assert!(reading.is_some());
    }
}
