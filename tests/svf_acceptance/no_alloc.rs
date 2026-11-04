use fugue::dsp::{BlockRamp, Svf};
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
fn running_the_filter_does_not_allocate() {
    let mut svf = Svf::new(48_000, 1000.0, 2.0);
    let mut peak = 0.0f32;
    let before = ALLOCS.with(Cell::get);
    for n in 0..4000 {
        // Ordinary, extreme and NaN arguments, interleaved with processing.
        match n % 5 {
            0 => svf.set(200.0 + (n % 900) as f32 * 10.0, 0.1 + (n % 40) as f32),
            1 => svf.set(1e9, 1000.0),
            2 => svf.set(f32::NAN, f32::NAN),
            3 => svf.set(5.0, -1.0),
            _ => {}
        }
        let x = (n as f32 * 0.07).sin();
        let o = svf.process(x);
        peak = peak
            .max(o.lowpass.abs())
            .max(o.bandpass.abs())
            .max(o.highpass.abs());
        peak = peak.max(o.notch().abs());
        let _ = (svf.cutoff(), svf.q());
    }
    svf.reset();
    assert_eq!(ALLOCS.with(Cell::get), before);
    assert!(peak.is_finite());
}

#[test]
fn running_the_ramp_does_not_allocate() {
    let mut ramp = BlockRamp::new(0.0);
    let mut out = vec![0.0f32; 333];
    let mut sum = 0.0f32;
    let before = ALLOCS.with(Cell::get);
    for (i, frames) in [1usize, 7, 64, 333, 64, 1].into_iter().enumerate() {
        let target = if i % 2 == 0 { 1.0 } else { -0.25 };
        let steps = ramp.steps(target, frames);
        let _ = steps.len();
        for v in steps {
            sum += v;
        }
        ramp.fill(-target, &mut out[..frames]);
        sum += out[frames - 1];
        ramp.snap(target);
        sum += ramp.value();
    }
    assert_eq!(ALLOCS.with(Cell::get), before);
    assert!(sum.is_finite());
}
