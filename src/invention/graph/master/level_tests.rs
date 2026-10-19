use super::*;
use crate::alloc_counter::allocator_events;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 128;

/// One block of `left`/`right` on fresh live observers, ending at `end`.
fn observe(observers: &MasterObservers, left: &[f32], right: &[f32], end: u64) {
    observers.observe(left, right, left.len(), end);
}

fn drain(observers: &MasterObservers) -> MasterLevels {
    observers.levels.drain(SAMPLE_RATE)
}

/// Observing is the audio thread's work: no allocation or free, and no
/// lock, so a sampler holding the drain lock cannot hold it up.
#[test]
fn observing_neither_allocates_nor_waits_for_the_sampler() {
    let observers = MasterObservers::live();
    let block = vec![0.5f32; FRAMES];
    let held = observers.levels.drain.lock().unwrap();
    let ((), allocs, frees) =
        allocator_events(|| observe(&observers, &block, &block, FRAMES as u64));
    assert_eq!((allocs, frees), (0, 0));
    drop(held);
    assert_eq!(drain(&observers).frames, FRAMES as u64);
}

#[test]
fn a_window_reads_the_peaks_of_every_block_since_the_last_drain() {
    let observers = MasterObservers::live();
    // A full-scale sine on the left and a quiet constant on the right, over
    // two blocks.
    let sine: Vec<f32> = (0..FRAMES)
        .map(|i| (std::f32::consts::TAU * i as f32 / FRAMES as f32).sin())
        .collect();
    let quiet = vec![-0.3f32; FRAMES];
    observe(&observers, &sine, &quiet, FRAMES as u64);
    observe(&observers, &sine, &quiet, 2 * FRAMES as u64);

    let levels = drain(&observers);
    assert!((levels.left_peak - 1.0).abs() < 1e-6, "{levels:?}");
    assert_eq!(levels.right_peak, 0.3);
    let two_blocks = 2 * FRAMES as u64;
    assert_eq!((levels.frames, levels.end_frame), (two_blocks, two_blocks));
    assert_eq!(levels.sample_rate, SAMPLE_RATE);

    // Draining started a new window.
    observe(&observers, &quiet, &quiet, 3 * FRAMES as u64);
    let next = drain(&observers);
    assert_eq!(next.left_peak, 0.3);
    assert_eq!(
        (next.frames, next.end_frame),
        (FRAMES as u64, 3 * FRAMES as u64)
    );
}

#[test]
fn a_window_without_audio_does_not_move_the_clock() {
    let observers = MasterObservers::live();
    let empty = drain(&observers);
    assert_eq!(
        (empty.left_peak, empty.frames, empty.end_frame),
        (0.0, 0, 0)
    );

    let block = vec![0.5f32; FRAMES];
    observe(&observers, &block, &block, 4_800);
    assert_eq!(drain(&observers).end_frame, 4_800);
    let idle = drain(&observers);
    assert_eq!((idle.frames, idle.end_frame), (0, 4_800));
    assert_eq!(idle.right_peak, 0.0);
}

#[test]
fn peaks_are_measured_before_the_clamp_and_skip_non_finite_audio() {
    let observers = MasterObservers::live();
    let over = vec![2.0f32; FRAMES];
    let mut broken = vec![0.5f32; FRAMES];
    broken[1] = f32::NAN;
    observe(&observers, &over, &broken, FRAMES as u64);
    let levels = drain(&observers);
    assert_eq!((levels.left_peak, levels.right_peak), (2.0, 0.5));
}

#[test]
fn the_window_end_converts_to_milliseconds_at_the_stated_rate() {
    let levels = |end_frame, sample_rate| MasterLevels {
        left_peak: 0.0,
        right_peak: 0.0,
        end_frame,
        frames: 0,
        sample_rate,
    };
    assert_eq!(levels(96_000, 48_000).end_ms(), 2_000.0);
    // Rounded to the microsecond: 4_864 frames at 48 kHz is 101.3333... ms.
    assert_eq!(levels(4_864, 48_000).end_ms(), 101.333);
    assert_eq!(levels(44_100, 44_100).end_ms(), 1_000.0);
}

/// Draining is the sampler's job; on the audio thread it would mean a
/// lock there, so debug builds refuse.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "control-only")]
fn draining_on_the_audio_thread_is_refused() {
    let observers = MasterObservers::live();
    let _audio_thread = crate::audio_thread::AudioThreadScope::enter();
    drain(&observers);
}
