//! The master level meter: peak and RMS of the mixed output, per meter
//! window, handed from the audio thread to an off-thread sampler.
//!
//! The audio thread folds each block's peak magnitudes, sums of squares and
//! frame count into one of two slots; a drain flips which slot it writes
//! and reads the other. Every block lands in exactly one window, whole, so a
//! reading's peak, RMS, frame count and end position always describe the
//! same samples, and consecutive windows tile the audio clock without a gap
//! or an overlap.
//!
//! # The handoff
//!
//! One word, `ctl`, holds the slot the audio thread writes (its low bit)
//! above a count the audio thread bumps by [`CLAIM`] as it starts folding a
//! block and again as it finishes, so the [`BUSY`] bit is set exactly while
//! a block is being folded:
//!
//! - **Audio thread** ([`LevelMeter::observe`]): `fetch_add(CLAIM, Acquire)`
//!   reads the slot and marks the fold busy in one step; it folds into that
//!   slot with plain relaxed loads and stores, then `fetch_add(CLAIM,
//!   Release)`. Two uncontended atomic adds a block: no lock, no wait, no
//!   allocation.
//! - **Sampler** ([`LevelMeter::drain`]): `fetch_xor(ACTIVE, AcqRel)` points
//!   the audio thread at the other slot. If the word it replaced was busy, a
//!   fold into the old slot started before the flip, so the sampler yields
//!   until the count moves on. Then it reads the old slot and zeroes it.
//!
//! All of these are read-modify-writes of one word, so they are totally
//! ordered: a fold that claimed before the flip is the busy one the sampler
//! waits for, and one that claimed after it writes the new slot. The release
//! that ends a fold, or the flip after a drain zeroed a slot, is what the
//! next side's acquire synchronizes with, so slot contents cross with it.
//! The `loom_tests` models check this file against loom's atomics.
//!
//! Drains are serialized by a sampler-side lock that the audio thread never
//! touches. Two samplers would split the windows between them, each seeing
//! only part of the output, so a runtime has one: the daemon's meter tick.

use super::sync::{debug_assert_control_thread, yield_now, AtomicU64, Mutex, Ordering};

/// The low bit of `ctl`: which slot the audio thread folds into.
const ACTIVE: u64 = 1;
/// What the audio thread adds to `ctl` as it starts and as it finishes a
/// fold. The count above [`ACTIVE`] is odd, so [`BUSY`] is set, during one.
const CLAIM: u64 = 2;
/// Set in `ctl` while the audio thread is folding a block.
const BUSY: u64 = CLAIM;

const MS_PER_SECOND: f64 = 1_000.0;
/// `audio_clock_ms` is rounded to the microsecond: finer than any window
/// and short on the wire.
const MICROS_PER_MS: f64 = 1_000.0;

/// One block's levels, measured on the audio thread.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct BlockLevels {
    pub(crate) left_peak: f32,
    pub(crate) right_peak: f32,
    pub(crate) left_sum_of_squares: f32,
    pub(crate) right_sum_of_squares: f32,
    pub(crate) frames: u32,
}

/// The master output's levels over one meter window: everything the audio
/// thread rendered between the previous drain and this one.
///
/// Values are linear amplitude, where 1.0 is full scale (0 dBFS), measured
/// on the mixed block before the output stage clamps it, so overs show above
/// 1.0. Channels are the stereo mix's left and right.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MasterLevels {
    /// Largest absolute sample in the window, left channel.
    pub left_peak: f32,
    /// Largest absolute sample in the window, right channel.
    pub right_peak: f32,
    /// Root mean square over the same window, left channel. `None` when the
    /// window holds no frames or its samples were not finite.
    pub left_rms: Option<f32>,
    /// Root mean square over the same window, right channel.
    pub right_rms: Option<f32>,
    /// The audio clock at the end of the window: frames the invention had
    /// rendered, counted from zero when it started playing. A window with no
    /// frames ends where the previous one did.
    pub end_frame: u64,
    /// Frames in the window, which started at `end_frame - frames`.
    pub frames: u64,
    /// The rate the audio clock counts frames at.
    pub sample_rate: u32,
}

impl MasterLevels {
    /// [`Self::end_frame`] in milliseconds at [`Self::sample_rate`], rounded
    /// to the microsecond.
    pub fn end_ms(&self) -> f64 {
        let ms = self.end_frame as f64 * MS_PER_SECOND / f64::from(self.sample_rate);
        (ms * MICROS_PER_MS).round() / MICROS_PER_MS
    }
}

/// One window's accumulators. Written only by the fold that holds the slot
/// and read only by the drain that owns it after a flip, so relaxed accesses
/// suffice: `ctl` orders who holds it.
#[derive(Default)]
struct Slot {
    /// Bits of the `f32` peaks.
    left_peak: AtomicU64,
    right_peak: AtomicU64,
    /// Bits of the `f64` sums of squares.
    left_sum: AtomicU64,
    right_sum: AtomicU64,
    frames: AtomicU64,
    end_frame: AtomicU64,
}

/// Peak and sums as read back from a slot.
struct Window {
    left_peak: f32,
    right_peak: f32,
    left_sum: f64,
    right_sum: f64,
    frames: u64,
    end_frame: u64,
}

fn load_f32(cell: &AtomicU64) -> f32 {
    f32::from_bits(cell.load(Ordering::Relaxed) as u32)
}

fn load_f64(cell: &AtomicU64) -> f64 {
    f64::from_bits(cell.load(Ordering::Relaxed))
}

impl Slot {
    /// Folds one block in. Only the fold holding this slot calls it.
    fn fold(&self, block: &BlockLevels, end_frame: u64) {
        let left_peak = load_f32(&self.left_peak).max(block.left_peak);
        let right_peak = load_f32(&self.right_peak).max(block.right_peak);
        let left_sum = load_f64(&self.left_sum) + f64::from(block.left_sum_of_squares);
        let right_sum = load_f64(&self.right_sum) + f64::from(block.right_sum_of_squares);
        let frames = self.frames.load(Ordering::Relaxed) + u64::from(block.frames);
        self.left_peak
            .store(u64::from(left_peak.to_bits()), Ordering::Relaxed);
        self.right_peak
            .store(u64::from(right_peak.to_bits()), Ordering::Relaxed);
        self.left_sum.store(left_sum.to_bits(), Ordering::Relaxed);
        self.right_sum.store(right_sum.to_bits(), Ordering::Relaxed);
        self.frames.store(frames, Ordering::Relaxed);
        self.end_frame.store(end_frame, Ordering::Relaxed);
    }

    /// Reads the window out and zeroes the slot for its next turn. Only the
    /// drain that just flipped away from this slot calls it.
    fn take(&self) -> Window {
        let window = Window {
            left_peak: load_f32(&self.left_peak),
            right_peak: load_f32(&self.right_peak),
            left_sum: load_f64(&self.left_sum),
            right_sum: load_f64(&self.right_sum),
            frames: self.frames.load(Ordering::Relaxed),
            end_frame: self.end_frame.load(Ordering::Relaxed),
        };
        // Zero is the bit pattern of 0.0 for both float widths.
        for cell in [
            &self.left_peak,
            &self.right_peak,
            &self.left_sum,
            &self.right_sum,
            &self.frames,
            &self.end_frame,
        ] {
            cell.store(0, Ordering::Relaxed);
        }
        window
    }
}

/// RMS from a window's sum of squares. It never exceeds the peak in exact
/// arithmetic; a constant-magnitude block (a square wave, say) can round
/// just above it, so it is capped there.
fn rms(sum_of_squares: f64, frames: u64, peak: f32) -> Option<f32> {
    if frames == 0 {
        return None;
    }
    let rms = (sum_of_squares / frames as f64).sqrt() as f32;
    rms.is_finite().then(|| rms.min(peak))
}

/// The meter shared by the audio thread, which folds blocks in, and the
/// sampler, which drains windows out. See the module docs for the handoff.
#[derive(Default)]
pub(crate) struct LevelMeter {
    ctl: AtomicU64,
    slots: [Slot; 2],
    /// Serializes drains, and holds where the last non-empty window ended.
    pub(super) drain: Mutex<u64>,
}

impl LevelMeter {
    /// Audio thread: folds one block that ends at `end_frame` on the audio
    /// clock. Wait-free and allocation-free.
    #[inline]
    pub(crate) fn observe(&self, block: &BlockLevels, end_frame: u64) {
        let claimed = self.ctl.fetch_add(CLAIM, Ordering::Acquire);
        self.slots[(claimed & ACTIVE) as usize].fold(block, end_frame);
        self.ctl.fetch_add(CLAIM, Ordering::Release);
    }

    /// Sampler: the levels since the previous drain, after which the next
    /// window starts. Never blocks the audio thread; it may yield for the
    /// length of one fold if it catches one mid-way.
    pub(crate) fn drain(&self, sample_rate: u32) -> MasterLevels {
        debug_assert_control_thread("draining the level meter");
        let mut last_end = self.drain.lock().unwrap_or_else(|e| e.into_inner());
        let flipped = self.ctl.fetch_xor(ACTIVE, Ordering::AcqRel);
        if flipped & BUSY != 0 {
            let count = flipped >> 1;
            while self.ctl.load(Ordering::Acquire) >> 1 == count {
                yield_now();
            }
        }
        let window = self.slots[(flipped & ACTIVE) as usize].take();
        if window.frames > 0 {
            *last_end = window.end_frame;
        }
        MasterLevels {
            left_peak: window.left_peak,
            right_peak: window.right_peak,
            left_rms: rms(window.left_sum, window.frames, window.left_peak),
            right_rms: rms(window.right_sum, window.frames, window.right_peak),
            end_frame: *last_end,
            frames: window.frames,
            sample_rate,
        }
    }
}
