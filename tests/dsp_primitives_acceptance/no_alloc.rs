use fugue::dsp::{Factor, Oversampler};
use fugue::MAX_BLOCK;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

// Counted per thread so concurrently running tests do not see each other.
thread_local!(static ALLOCS: Cell<usize> = const { Cell::new(0) });

struct Counting;

// `alloc_zeroed` and `realloc` default to calling `alloc`, so they are counted too.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

#[test]
fn process_does_not_allocate() {
    for factor in [Factor::Two, Factor::Four] {
        for len in [1, 64, MAX_BLOCK] {
            let mut os = Oversampler::new(factor, 48_000);
            let mut block = vec![0.25f32; len];
            let before = ALLOCS.with(Cell::get);
            for _ in 0..8 {
                os.process(&mut block, |hi| hi.iter_mut().for_each(|s| *s = s.tanh()));
            }
            assert_eq!(ALLOCS.with(Cell::get), before, "{factor:?}, block of {len}");
        }
    }
}
