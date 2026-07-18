//! WSOLA elastic reader: independent time-stretch and pitch-shift.
//!
//! The reader synthesizes output one window ("cycle") at a time. Each cycle
//! reads a windowed segment of the source at the pitch step, crossfades it
//! against the tail of the previous cycle, and remembers the segment's
//! natural continuation as the next tail. A bounded alignment search keeps
//! successive windows phase-coherent: candidate offsets around the nominal
//! head are scored by normalized cross-correlation against the tail on a
//! precomputed mono mix (a strided sweep of the whole range, then
//! sample-exact refinement around the winner).
//!
//! Time and pitch are decoupled by construction: the nominal source head
//! advances by `time_ratio` per emitted frame (so a region of `L` source
//! frames always occupies `ceil(L / time_ratio)` output frames), while reads
//! inside a window step by `pitch_ratio` through the shared cubic kernel.
//!
//! # Why the search is shaped the way it is
//!
//! The tail is the exact continuation of what was just played, so off unity
//! the search would happily "chase" it — playing at native rate for a cycle
//! or two, then snapping back to nominal — which turns a steady stretch into
//! an audible rate sawtooth with a discontinuity at every resync. Two
//! defenses keep the search a *phase* aligner instead of a rate cheat:
//!
//! - Geometry: the cycle is long relative to the seek radius, so for
//!   `|1 - time_ratio| >= SEEK_SECS / CYCLE_SECS` the continuation falls
//!   outside the search range entirely and every window re-anchors near the
//!   nominal head.
//! - A small quadratic penalty on offset magnitude breaks the near-ties in
//!   the mild-ratio band in favor of staying anchored. At unity the
//!   continuation sits at offset zero, is never penalized, and playback
//!   reconstructs the source exactly.
//!
//! Split of work across threads:
//! - [`ElasticAnalysis::analyze`] allocates and is called off the audio
//!   thread (module build / `set_source`); the result is cached per asset on
//!   [`SampleData`].
//! - [`ElasticReader`] methods called from `process()` are allocation-free
//!   and lock-free: every buffer is sized at construction.

use super::SampleData;

/// Crossfade length between successive windows.
const OVERLAP_SECS: f32 = 0.010;

/// Output frames emitted per synthesis cycle (includes the crossfade).
/// Deliberately long relative to [`SEEK_SECS`]; see the module docs.
const CYCLE_SECS: f32 = 0.060;

/// Maximum alignment offset searched around the nominal head, each way.
const SEEK_SECS: f32 = 0.012;

/// Correlation-score penalty at full seek deviation. Scores are normalized
/// to [-1, 1], so this is directly comparable to correlation differences:
/// big enough to break near-ties against far offsets, small enough that a
/// genuinely better phase alignment still wins.
const SEEK_PENALTY: f32 = 0.1;

/// Floor for both ratios so the read head always moves forward; zero or
/// negative ratios would stall the reader.
const MIN_RATIO: f32 = 1e-4;

/// Per-asset analysis backing the alignment search: the full-rate mono mix,
/// precomputed so the audio thread never averages channels per candidate.
/// Immutable after construction, so readers share it by reference.
pub(crate) struct ElasticAnalysis {
    mono: Vec<f32>,
}

impl ElasticAnalysis {
    pub(crate) fn analyze(sample: &SampleData) -> Self {
        let len = sample.len();
        let mut mono = Vec::with_capacity(len);
        for i in 0..len {
            mono.push(0.5 * (sample.left[i] + sample.right[i]));
        }
        Self { mono }
    }
}

/// One voice of elastic playback over a source region. Modules own one per
/// playback voice and drive it a frame at a time from `process()`.
pub(crate) struct ElasticReader {
    overlap: usize,
    cycle: usize,
    seek: usize,
    /// Raised-cosine 0→1 ramp; the complementary fade is `1 - fade_in[k]`.
    fade_in: Vec<f32>,
    out_left: Vec<f32>,
    out_right: Vec<f32>,
    tail_left: Vec<f32>,
    tail_right: Vec<f32>,
    tail_mono: Vec<f32>,
    out_len: usize,
    out_pos: usize,
    /// Nominal source head in frames; advances by `time_ratio` per emitted
    /// frame regardless of pitch, so callers key gates and loops off it.
    src_head: f64,
    /// Aligned read base of the most recent window (test observability).
    last_base: f64,
    /// A fresh (post-hard-reset) window starts at full amplitude instead of
    /// crossfading, preserving transients at slice/sample starts.
    fresh: bool,
    /// The alignment search only runs once a tail from this region exists.
    started: bool,
}

impl ElasticReader {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let overlap = ((rate * OVERLAP_SECS) as usize).max(16);
        let cycle = ((rate * CYCLE_SECS) as usize).max(overlap * 2);
        // Even, so the strided offset sweep lands on zero exactly.
        let seek = (((rate * SEEK_SECS) as usize) / 2).max(1) * 2;
        let fade_in = (0..overlap)
            .map(|k| {
                let t = (k as f32 + 0.5) / overlap as f32;
                0.5 - 0.5 * (std::f32::consts::PI * t).cos()
            })
            .collect();
        Self {
            overlap,
            cycle,
            seek,
            fade_in,
            out_left: vec![0.0; cycle],
            out_right: vec![0.0; cycle],
            tail_left: vec![0.0; overlap],
            tail_right: vec![0.0; overlap],
            tail_mono: vec![0.0; overlap],
            out_len: 0,
            out_pos: 0,
            src_head: 0.0,
            last_base: 0.0,
            fresh: true,
            started: false,
        }
    }

    /// Hard reset to `start`: forgets the previous tail and starts the next
    /// window at full amplitude. Use when playback begins from silence.
    pub(crate) fn reset(&mut self, start: f64) {
        self.tail_left.fill(0.0);
        self.tail_right.fill(0.0);
        self.tail_mono.fill(0.0);
        self.reset_crossfade(start);
        self.fresh = true;
    }

    /// Repositions to `start` but keeps the previous tail, so the next
    /// window crossfades out of the old audio instead of cutting. Use for
    /// loop wraps and retriggers while still audible.
    pub(crate) fn reset_crossfade(&mut self, start: f64) {
        self.out_len = 0;
        self.out_pos = 0;
        self.src_head = start;
        self.fresh = false;
        self.started = false;
    }

    pub(crate) fn source_position(&self) -> f64 {
        self.src_head
    }

    /// Aligned read base of the most recent synthesized window. Test-only:
    /// lets stretch-smoothness tests observe how windows advance.
    #[cfg(test)]
    pub(crate) fn last_base(&self) -> f64 {
        self.last_base
    }

    /// Emits one output frame, or `None` once the nominal head has passed
    /// `region_end`. Allocation-free; synthesis of a new window happens
    /// in-place every `cycle` frames.
    pub(crate) fn next(
        &mut self,
        sample: &SampleData,
        analysis: &ElasticAnalysis,
        region_start: f64,
        region_end: f64,
        time_ratio: f32,
        pitch_ratio: f32,
    ) -> Option<(f32, f32)> {
        if self.src_head >= region_end {
            return None;
        }
        if self.out_pos >= self.out_len {
            self.synthesize(sample, analysis, region_start, region_end, pitch_ratio);
        }
        let frame = (self.out_left[self.out_pos], self.out_right[self.out_pos]);
        self.out_pos += 1;
        self.src_head += f64::from(time_ratio.max(MIN_RATIO));
        Some(frame)
    }

    fn synthesize(
        &mut self,
        sample: &SampleData,
        analysis: &ElasticAnalysis,
        region_start: f64,
        region_end: f64,
        pitch_ratio: f32,
    ) {
        let pitch = f64::from(pitch_ratio.max(MIN_RATIO));
        let base = if self.started {
            let offset = self.best_offset(analysis, region_start, region_end, pitch);
            (self.src_head + offset as f64).max(region_start)
        } else {
            self.src_head.max(region_start)
        };
        self.last_base = base;

        let blend = !self.fresh;
        for k in 0..self.cycle {
            let (l, r) = read_region(sample, base + k as f64 * pitch, region_start, region_end);
            if blend && k < self.overlap {
                let fade = self.fade_in[k];
                self.out_left[k] = self.tail_left[k] * (1.0 - fade) + l * fade;
                self.out_right[k] = self.tail_right[k] * (1.0 - fade) + r * fade;
            } else {
                self.out_left[k] = l;
                self.out_right[k] = r;
            }
        }

        // Remember this window's natural continuation; the next cycle's
        // search aligns its window against it.
        for k in 0..self.overlap {
            let pos = base + (self.cycle + k) as f64 * pitch;
            let (l, r) = read_region(sample, pos, region_start, region_end);
            self.tail_left[k] = l;
            self.tail_right[k] = r;
            self.tail_mono[k] = 0.5 * (l + r);
        }

        self.out_len = self.cycle;
        self.out_pos = 0;
        self.fresh = false;
        self.started = true;
    }

    /// Search for the alignment offset (in source frames) whose window best
    /// continues the previous tail: a strided sweep of the full ±seek range,
    /// then sample-exact refinement around the winner. Both passes run at
    /// full rate on the precomputed mono mix.
    fn best_offset(
        &self,
        analysis: &ElasticAnalysis,
        region_start: f64,
        region_end: f64,
        pitch: f64,
    ) -> isize {
        let seek = self.seek as isize;
        let mut best = 0isize;
        let mut best_score = f32::MIN;

        let sweep_energy = tap_energy(&self.tail_mono, 2);
        let mut d = -seek;
        while d <= seek {
            if let Some(score) = self.score(
                analysis,
                d,
                region_start,
                region_end,
                pitch,
                2,
                sweep_energy,
            ) {
                if score > best_score {
                    best_score = score;
                    best = d;
                }
            }
            d += 2;
        }

        let center = best;
        let refine_energy = tap_energy(&self.tail_mono, 1);
        best_score = f32::MIN;
        for d in (center - 2)..=(center + 2) {
            if d.abs() > seek {
                continue;
            }
            if let Some(score) = self.score(
                analysis,
                d,
                region_start,
                region_end,
                pitch,
                1,
                refine_energy,
            ) {
                if score > best_score {
                    best_score = score;
                    best = d;
                }
            }
        }
        best
    }

    /// Penalized correlation score for one candidate offset, or `None` when
    /// the candidate falls outside the region.
    #[allow(clippy::too_many_arguments)]
    fn score(
        &self,
        analysis: &ElasticAnalysis,
        d: isize,
        region_start: f64,
        region_end: f64,
        pitch: f64,
        stride: usize,
        reference_energy: f32,
    ) -> Option<f32> {
        let start = self.src_head + d as f64;
        if start < region_start || start >= region_end {
            return None;
        }
        let correlation = correlate(
            &self.tail_mono,
            &analysis.mono,
            start,
            pitch,
            stride,
            reference_energy,
        );
        let deviation = d as f32 / self.seek as f32;
        Some(correlation - SEEK_PENALTY * deviation * deviation)
    }
}

/// Sum of squares over every `stride`-th tap of `reference`.
fn tap_energy(reference: &[f32], stride: usize) -> f32 {
    reference.iter().step_by(stride).map(|v| v * v).sum()
}

/// Fully normalized cross-correlation (in [-1, 1]) of every `stride`-th tap
/// of `reference` against `signal` read from fractional `start` with `step`
/// between taps. Nearest-neighbour reads: alignment scoring doesn't need
/// interpolation accuracy. Full normalization keeps scores comparable to the
/// absolute deviation penalty regardless of signal level.
fn correlate(
    reference: &[f32],
    signal: &[f32],
    start: f64,
    step: f64,
    stride: usize,
    reference_energy: f32,
) -> f32 {
    let mut dot = 0.0f32;
    let mut energy = 0.0f32;
    for (k, &r) in reference.iter().enumerate().step_by(stride) {
        let idx = (start + k as f64 * step + 0.5) as usize;
        let v = signal.get(idx).copied().unwrap_or(0.0);
        dot += r * v;
        energy += v * v;
    }
    let norm = energy * reference_energy;
    if norm > 1e-12 {
        dot / norm.sqrt()
    } else {
        0.0
    }
}

/// Region-bounded stereo read: positions outside `[region_start, region_end)`
/// are silence, so window reads near a region edge fade naturally instead of
/// bleeding into neighbouring slices.
#[inline]
fn read_region(sample: &SampleData, pos: f64, region_start: f64, region_end: f64) -> (f32, f32) {
    if pos < region_start || pos >= region_end {
        return (0.0, 0.0);
    }
    sample.sample_at(pos)
}
