use fugue::dsp::{BandLimitedOscillator, Shape};
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
fn running_oscillators_does_not_allocate() {
    let mut osc = BandLimitedOscillator::new();
    let mut peak = 0.0f32;
    let before = ALLOCS.with(Cell::get);
    for shape in [Shape::Sine, Shape::Saw, Shape::Pulse, Shape::Triangle] {
        // Ordinary, zero, over-Nyquist increments; pulse widths at both extremes.
        for (inc, width) in [(0.04, 0.5), (0.0, 0.5), (0.7, 0.5), (0.1, 0.0), (0.1, 1.0)] {
            for n in 0..2000 {
                let v = match n % 7 {
                    0 => osc.next_synced(shape, inc, width, 0.3),
                    _ => osc.next(shape, inc, width),
                };
                peak = peak.max(v.abs());
                let _ = (osc.phase(), osc.wrapped());
            }
        }
    }
    osc.reset();
    assert_eq!(ALLOCS.with(Cell::get), before);
    assert!(peak.is_finite() && peak < 2.0);
}
