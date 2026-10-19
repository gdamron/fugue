//! Exhaustive interleaving models of the level meter's slot handoff.
//!
//! `levels.rs` is compiled a second time here against loom's atomics,
//! mutex and yield (the `sync` module below replaces the master module's
//! `std` one), so the models check the code that ships. Each explores the
//! interleavings and weak-memory outcomes loom models between an audio
//! thread folding blocks and a sampler draining windows, and asserts what
//! the handoff promises: every block lands in exactly one window, whole,
//! and consecutive windows tile the audio clock.
//!
//! The slot fields are atomics with relaxed accesses, so a broken handoff
//! is not a data race loom would flag but a stale or torn window, which the
//! assertions catch.
// The source file is deliberately loaded a second time, and not every item
// it defines is exercised here.
#![allow(clippy::duplicate_mod, dead_code)]

mod sync {
    pub(super) use loom::sync::atomic::Ordering;
    pub(super) use loom::sync::Mutex;
    pub(super) use loom::thread::yield_now;

    /// The audio-thread scope is not modeled.
    pub(super) fn debug_assert_control_thread(_what: &str) {}

    /// loom's `AtomicU64`, with every access also made a `Relaxed` RMW of a
    /// shadow atomic. loom remembers only the last access to an atomic, so
    /// when one thread loads a slot field and another then loads and stores
    /// it, the schedule where the store precedes the first load is never
    /// explored; the shadow makes any two accesses dependent, so loom
    /// explores both orders. `Relaxed` RMWs create no happens-before edge,
    /// so this adds schedules without hiding a stale read. (The request
    /// queue's loom models use the same shim, for the same reason.)
    #[derive(Default)]
    pub(super) struct AtomicU64 {
        value: loom::sync::atomic::AtomicU64,
        shadow: loom::sync::atomic::AtomicU64,
    }

    impl AtomicU64 {
        fn conflict(&self) {
            self.shadow.fetch_add(1, Ordering::Relaxed);
        }

        pub(super) fn load(&self, order: Ordering) -> u64 {
            self.conflict();
            self.value.load(order)
        }

        pub(super) fn store(&self, value: u64, order: Ordering) {
            self.conflict();
            self.value.store(value, order);
        }

        pub(super) fn fetch_add(&self, value: u64, order: Ordering) -> u64 {
            self.conflict();
            self.value.fetch_add(value, order)
        }

        pub(super) fn fetch_xor(&self, value: u64, order: Ordering) -> u64 {
            self.conflict();
            self.value.fetch_xor(value, order)
        }
    }
}

#[path = "levels.rs"]
mod levels;

use levels::{BlockLevels, LevelMeter, MasterLevels};
use loom::sync::Arc;
use loom::thread;

const SAMPLE_RATE: u32 = 48_000;
/// Each modeled block's sum of squares for its one frame: an RMS of 0.5,
/// below every block's peak, so the RMS cap never hides a torn window.
const BLOCK_SUM_OF_SQUARES: f32 = 0.25;
const BLOCK_RMS: f32 = 0.5;
/// Bounds preemptions to keep each model to seconds; three are enough for a
/// drain to land inside a fold, and a second drain inside the next.
const PREEMPTIONS: usize = 3;

/// Block `n` (from 1): one frame ending at frame `n`, with peak `n`, so a
/// window's peak names its last block and its end frame must match.
fn block(n: u64) -> BlockLevels {
    BlockLevels {
        left_peak: n as f32,
        right_peak: n as f32,
        left_sum_of_squares: BLOCK_SUM_OF_SQUARES,
        right_sum_of_squares: BLOCK_SUM_OF_SQUARES,
        frames: 1,
    }
}

/// Asserts that `readings`, in drain order, are whole windows that tile the
/// clock from zero through block `blocks`, each block exactly once.
fn assert_tiled(readings: &[MasterLevels], blocks: u64) {
    let mut end = 0;
    for reading in readings {
        assert_eq!(reading.end_frame - reading.frames, end, "{readings:?}");
        if reading.frames == 0 {
            assert_eq!(reading.left_peak, 0.0, "{readings:?}");
            assert_eq!(reading.left_rms, None, "{readings:?}");
        } else {
            // The window's latest block has the largest peak, and every
            // block's sum came with its frame.
            assert_eq!(reading.left_peak, reading.end_frame as f32, "{readings:?}");
            assert_eq!(reading.right_peak, reading.end_frame as f32, "{readings:?}");
            assert_eq!(reading.left_rms, Some(BLOCK_RMS), "{readings:?}");
            assert_eq!(reading.right_rms, Some(BLOCK_RMS), "{readings:?}");
        }
        end = reading.end_frame;
    }
    assert_eq!(end, blocks, "{readings:?}");
}

/// Spawns an audio thread that folds blocks 1 through `blocks`.
fn spawn_audio(meter: &Arc<LevelMeter>, blocks: u64) -> thread::JoinHandle<()> {
    let meter = meter.clone();
    thread::spawn(move || {
        for n in 1..=blocks {
            meter.observe(&block(n), n);
        }
    })
}

/// A sampler draining while the audio thread folds: whatever the
/// interleaving, every block lands in exactly one window, whole, and the
/// windows tile the clock.
#[test]
fn drains_racing_folds_tile_the_clock_with_whole_blocks() {
    const BLOCKS: u64 = 2;
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(PREEMPTIONS);
    model.check(|| {
        let meter = Arc::new(LevelMeter::default());
        let audio = spawn_audio(&meter, BLOCKS);
        let mut readings = vec![meter.drain(SAMPLE_RATE), meter.drain(SAMPLE_RATE)];
        audio.join().unwrap();
        readings.push(meter.drain(SAMPLE_RATE));
        assert_tiled(&readings, BLOCKS);
    });
}

/// More blocks than drains, so the audio thread reuses a slot the sampler
/// zeroed: the zeroing reaches the audio thread before its next fold there.
#[test]
fn a_reused_slot_starts_from_zero() {
    const BLOCKS: u64 = 3;
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(PREEMPTIONS);
    model.check(|| {
        let meter = Arc::new(LevelMeter::default());
        let audio = spawn_audio(&meter, BLOCKS);
        let mut readings = vec![meter.drain(SAMPLE_RATE), meter.drain(SAMPLE_RATE)];
        audio.join().unwrap();
        readings.push(meter.drain(SAMPLE_RATE));
        assert_tiled(&readings, BLOCKS);
    });
}

/// Two samplers draining at once are serialized by the drain lock: between
/// them they still see every block exactly once, in whole windows.
#[test]
fn concurrent_drains_split_windows_without_losing_blocks() {
    const BLOCKS: u64 = 2;
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(PREEMPTIONS);
    model.check(|| {
        let meter = Arc::new(LevelMeter::default());
        let audio = spawn_audio(&meter, BLOCKS);
        let other = {
            let meter = meter.clone();
            thread::spawn(move || meter.drain(SAMPLE_RATE))
        };
        let mine = meter.drain(SAMPLE_RATE);
        let theirs = other.join().unwrap();
        audio.join().unwrap();
        let rest = meter.drain(SAMPLE_RATE);
        // The two concurrent drains happened in some order; either way the
        // sequence must tile.
        let mut readings = vec![mine, theirs];
        readings.sort_by_key(|r| (r.end_frame - r.frames, r.end_frame));
        readings.push(rest);
        assert_tiled(&readings, BLOCKS);
    });
}
