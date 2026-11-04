//! Oscillators whose edges don't alias.
//!
//! A sawtooth or pulse computed straight from its phase ("naive") jumps
//! between two samples. That jump carries harmonics far above Nyquist,
//! which fold back as inharmonic tones: a high saw sounds whistly and
//! out of tune. A triangle's corners do the same, more quietly.
//!
//! The polynomial band-limited step (polyBLEP, after Välimäki and
//! colleagues) fixes the samples around every jump with a short
//! polynomial: the residual between the hard step and one smoothed by a
//! kernel. Its integral (polyBLAMP) does the same for a corner. The kernel
//! here is the cubic B-spline, which spans four samples. That holds
//! aliases about 10 dB lower than the two-sample triangle kernel, at the
//! cost of a rolloff in the top octaves: at 48 kHz about 2.5 dB at
//! 10 kHz, 6 dB at 15 kHz and 10.5 dB at 20 kHz (a shelf can restore it).
//! The fix reaches two samples before each jump, so the oscillator holds
//! its output back two samples.

use super::sine_table::{cosine, sine};

/// The waveforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Sine,
    /// Rises from −1 to 1 across the cycle, then drops.
    Saw,
    /// 1 for the first `pulse_width` of the cycle, −1 for the rest.
    Pulse,
    /// Rises from −1 to 1 over the first half of the cycle and falls over
    /// the second.
    Triangle,
}

/// The highest increment (frequency over sample rate): just under
/// Nyquist.
const MAX_INCREMENT: f32 = 0.499;

/// The latest a sync may fall: just after the previous sample, which was
/// computed before it.
const LATEST_SYNC: f32 = 1.0 - f32::EPSILON;

/// A band-limited oscillator. Each sample comes out two samples late.
///
/// Plain arithmetic, no tables beyond the shared sine and no heap: safe to
/// run on the audio thread.
///
/// The phase only runs forward (a negative increment holds it still), so
/// through-zero FM needs another approach. A jump from changing `shape`
/// between samples is not smoothed: crossfade instead. Pulse-width
/// changes are smoothed, however fast.
#[derive(Clone, Copy, Debug, Default)]
pub struct BandLimitedOscillator {
    /// Position in the cycle, in `[0, 1)`.
    phase: f32,
    /// The two previous samples, oldest first, still open to corrections.
    held: [f32; 2],
    /// A correction waiting for the next sample.
    ahead: f32,
    /// If the last sample started a new cycle: how long before the sample
    /// it did, in samples.
    wrapped: Option<f32>,
    /// The pulse width of the last sample, once there is one.
    width: Option<f32>,
}

/// Corrections from the jumps and corners found in one step, for the
/// samples two and one before the new one, the new one, and the next.
#[derive(Default)]
struct Corrections([f32; 4]);

impl Corrections {
    /// A jump of `height`, `since` samples (`0..=1`) before the new
    /// sample.
    #[inline]
    fn step(&mut self, since: f32, height: f32) {
        for (k, fix) in self.0.iter_mut().enumerate() {
            *fix += height * step_residual(since + k as f32 - 2.0);
        }
    }

    /// A change of `slope` (per sample) `since` samples before the new
    /// sample.
    #[inline]
    fn corner(&mut self, since: f32, slope: f32) {
        for (k, fix) in self.0.iter_mut().enumerate() {
            *fix += slope * corner_residual(since + k as f32 - 2.0);
        }
    }
}

/// The cubic B-spline kernel's smoothed unit step, less the hard one, `x`
/// samples after the step (zero beyond two samples either side).
#[inline]
fn step_residual(x: f32) -> f32 {
    // The smoothed step's rise before the jump; after it, the residual is
    // the mirror image, negated.
    let rise = |x: f32| {
        if x <= -2.0 {
            0.0
        } else if x <= -1.0 {
            let u = x + 2.0;
            u * u * u * u / 24.0
        } else {
            0.5 + x * (2.0 / 3.0 + x * x * (-1.0 / 3.0 - x / 8.0))
        }
    };
    if x < 0.0 {
        rise(x)
    } else {
        -rise(-x)
    }
}

/// The integral of [`step_residual`]: the smoothed unit corner less the
/// hard one, `x` samples after the corner. It is even in `x`.
#[inline]
fn corner_residual(x: f32) -> f32 {
    let x = -x.abs();
    if x <= -2.0 {
        0.0
    } else if x <= -1.0 {
        let u = x + 2.0;
        u * u * u * u * u / 120.0
    } else {
        7.0 / 30.0 + x * (0.5 + x * (1.0 / 3.0 + x * x * (-1.0 / 12.0 - x / 40.0)))
    }
}

impl BandLimitedOscillator {
    /// An oscillator at the start of its cycle.
    pub fn new() -> Self {
        Self::default()
    }

    /// Position in the cycle of the sample last computed (which comes out
    /// two samples later), in `[0, 1)`. Each call advances the phase first,
    /// then computes the sample there.
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// If the last sample started a new cycle (or was synced), how many
    /// samples (`0..1`) before it the cycle began: pass this to a synced
    /// oscillator's [`next_synced`](Self::next_synced).
    pub fn wrapped(&self) -> Option<f32> {
        self.wrapped
    }

    /// Back to the start of the cycle, with no sample held: for a fresh
    /// start from silence. Mid-sound it clicks; to restart the cycle
    /// without one, call [`next_synced`](Self::next_synced) with `since`
    /// 0.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The next sample of `shape` at `increment` (frequency over sample
    /// rate, held to `0..0.5`). `pulse_width` (`0..=1`) shapes the pulse.
    /// An input that is not a number counts as 0 (0.5 for the width).
    #[inline]
    pub fn next(&mut self, shape: Shape, increment: f32, pulse_width: f32) -> f32 {
        let increment = clean(increment, 0.0).clamp(0.0, MAX_INCREMENT);
        let width = clean(pulse_width, 0.5).clamp(0.0, 1.0);
        let start_width = self.width.unwrap_or(width);
        let mut fix = Corrections::default();
        self.wrapped = None;
        let move_ = Move {
            from: self.phase,
            to: self.phase + increment,
            widths: (start_width, width),
            span: 1.0,
            late: 0.0,
        };
        move_.collect(&mut fix, shape, increment);
        self.phase = if move_.to >= 1.0 {
            let phase = move_.to - 1.0;
            self.wrapped = Some(phase / increment);
            phase
        } else {
            move_.to
        };
        self.emit(fix, shape, width)
    }

    /// Like [`next`](Self::next), but the cycle restarts (hard sync)
    /// `since` samples (`0..1`) before this sample, as a master's
    /// [`wrapped`](Self::wrapped) reports.
    #[inline]
    pub fn next_synced(
        &mut self,
        shape: Shape,
        increment: f32,
        pulse_width: f32,
        since: f32,
    ) -> f32 {
        let increment = clean(increment, 0.0).clamp(0.0, MAX_INCREMENT);
        let width = clean(pulse_width, 0.5).clamp(0.0, 1.0);
        let since = clean(since, 0.0).clamp(0.0, LATEST_SYNC);
        let start_width = self.width.unwrap_or(width);
        let sync_width = start_width + (width - start_width) * (1.0 - since);
        let mut fix = Corrections::default();
        // Up to the moment of the sync, the cycle runs as usual.
        let before = Move {
            from: self.phase,
            to: self.phase + increment * (1.0 - since),
            widths: (start_width, sync_width),
            span: 1.0 - since,
            late: since,
        };
        before.collect(&mut fix, shape, increment);
        let at = before.to - before.to.floor();
        // Then it jumps back to the start: in value, and in slope.
        let jump = value(shape, 0.0, sync_width) - value(shape, at, sync_width);
        fix.step(since, jump);
        fix.corner(since, (slope(shape, 0.0) - slope(shape, at)) * increment);
        // Less than half a cycle remains, so this cannot wrap again.
        let after = Move {
            from: 0.0,
            to: increment * since,
            widths: (sync_width, width),
            span: since,
            late: 0.0,
        };
        after.collect(&mut fix, shape, increment);
        self.phase = after.to;
        self.wrapped = Some(since);
        self.emit(fix, shape, width)
    }

    /// Releases the held sample with its corrections and holds the new one.
    #[inline]
    fn emit(&mut self, fix: Corrections, shape: Shape, width: f32) -> f32 {
        let [two_back, one_back, now, next] = fix.0;
        let out = self.held[0] + two_back;
        self.held = [
            self.held[1] + one_back,
            value(shape, self.phase, width) + self.ahead + now,
        ];
        self.ahead = next;
        self.width = Some(width);
        out
    }
}

/// `value` unless it is not a number.
#[inline]
fn clean(value: f32, otherwise: f32) -> f32 {
    if value.is_nan() {
        otherwise
    } else {
        value
    }
}

/// One straight run of the phase within a sample: from `from` to `to`
/// (unwrapped), while the pulse width moves from `widths.0` to
/// `widths.1`, taking `span` samples and ending `late` samples before the
/// new sample.
struct Move {
    from: f32,
    to: f32,
    widths: (f32, f32),
    span: f32,
    late: f32,
}

impl Move {
    /// How long before the new sample the run passes the point a fraction
    /// `along` of the way through it.
    #[inline]
    fn since(&self, along: f32) -> f32 {
        ((1.0 - along) * self.span + self.late).min(LATEST_SYNC)
    }

    /// When the phase passes the fixed point `edge`, how long before the
    /// new sample.
    #[inline]
    fn passes(&self, edge: f32) -> Option<f32> {
        (self.from < edge && edge <= self.to)
            .then(|| self.since((edge - self.from) / (self.to - self.from)))
    }

    /// Adds the corrections for every jump and corner of `shape` the run
    /// passes.
    #[inline]
    fn collect(&self, fix: &mut Corrections, shape: Shape, increment: f32) {
        match shape {
            Shape::Sine => {}
            Shape::Saw => {
                if let Some(since) = self.passes(1.0) {
                    fix.step(since, -2.0);
                }
            }
            Shape::Pulse => self.pulse_edges(fix),
            Shape::Triangle => {
                for (edge, turn) in [(0.5, -8.0), (1.0, 8.0)] {
                    if let Some(since) = self.passes(edge) {
                        fix.corner(since, turn * increment);
                    }
                }
            }
        }
    }

    /// A pulse jumps when the phase starts a cycle, and whenever the phase
    /// and the width pass each other: the phase overtaking the width (the
    /// fall) or the width overtaking the phase (a rise).
    #[inline]
    fn pulse_edges(&self, fix: &mut Corrections) {
        // The pulse is `2·(⌊phase⌋ − ⌊phase − width⌋) − 1`: it rises 2 when
        // the first term steps up (a new cycle), falls 2 when the second
        // steps up (the phase overtakes the width) and rises 2 when it steps
        // down (the width overtakes the phase). At width 0 or 1 the steps
        // cancel.
        let (start, end) = self.widths;
        if let Some(since) = self.passes(1.0) {
            fix.step(since, 2.0);
        }
        // How far the phase is past the width, at the start and end.
        let (lead_start, lead_end) = (self.from - start, self.to - end);
        for cycle in [0.0, 1.0] {
            let rises = lead_end < cycle && cycle <= lead_start;
            if (lead_start < cycle && cycle <= lead_end) || rises {
                let along = (cycle - lead_start) / (lead_end - lead_start);
                fix.step(self.since(along), if rises { 2.0 } else { -2.0 });
            }
        }
    }
}

/// The waveform's value at `phase`, before any smoothing.
#[inline]
fn value(shape: Shape, phase: f32, width: f32) -> f32 {
    match shape {
        Shape::Sine => sine(phase),
        Shape::Saw => 2.0 * phase - 1.0,
        Shape::Pulse => {
            if phase < width {
                1.0
            } else {
                -1.0
            }
        }
        Shape::Triangle => {
            if phase < 0.5 {
                4.0 * phase - 1.0
            } else {
                3.0 - 4.0 * phase
            }
        }
    }
}

/// The waveform's slope at `phase`, per cycle.
#[inline]
fn slope(shape: Shape, phase: f32) -> f32 {
    match shape {
        Shape::Sine => std::f32::consts::TAU * cosine(phase),
        Shape::Saw => 2.0,
        Shape::Pulse => 0.0,
        Shape::Triangle => {
            if phase < 0.5 {
                4.0
            } else {
                -4.0
            }
        }
    }
}

#[cfg(test)]
#[path = "band_limited_tests.rs"]
mod tests;
