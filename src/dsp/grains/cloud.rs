//! Granular playback: a fixed pool of grains, each a windowed, re-pitched
//! read of the recording.
//!
//! Ported from Mutable Instruments' `clouds/dsp/grain.h` and
//! `clouds/dsp/granular_sample_player.h` (MIT, Copyright 2014 Emilie
//! Gillet), adapted to Fugue: grains are scheduled and played a sample at a
//! time (so the output doesn't depend on the block size), sizes are in
//! seconds rather than samples at 32 kHz, the window is computed rather
//! than read from a table, and grains can be started by a clock. See
//! THIRD_PARTY_NOTICES.md.
//!
//! Each grain fixes its position, length, pitch and pan when it starts, so
//! moving those controls changes the grains that follow without touching
//! the ones already playing: no control steps the sound.

use super::Recording;
use crate::dsp::{cosine, Rng};

/// The most grains playing at once.
pub const MAX_GRAINS: usize = 32;
/// The shortest grain (`size` 0), in seconds; `size` 1 is 16 times longer.
const SHORTEST_GRAIN: f32 = 0.032;
/// Frames kept between a grain's reads and the write head.
const MARGIN: f32 = 4.0;
/// How far ahead of the write head an interpolated read reaches, in frames.
const READ_REACH: f32 = 3.0;
/// The longest and shortest early release of a grain the write head will
/// overrun.
const RELEASE_SECONDS: f32 = 0.005;
const SHORTEST_RELEASE_SECONDS: f32 = 0.001;
/// Smoothed levels below this are silence: flushed, so they never decay
/// into subnormal numbers.
const SILENT: f32 = 1e-20;

/// What the next grains are made of.
#[derive(Clone, Copy, Default)]
pub struct GrainShape {
    /// 0 reads the newest audio, 1 the oldest.
    pub position: f32,
    /// 0 to 1, a grain of 32 ms up to 16 times that (0.5 s).
    pub size: f32,
    /// Playback rate (2 an octave up).
    pub ratio: f32,
    /// 0 to 1, near-rectangular to a smooth bell.
    pub window: f32,
    /// 0 to 1, how far a grain may be panned from the centre.
    pub spread: f32,
}

/// When grains start, for one frame.
#[derive(Clone, Copy)]
pub enum Onsets {
    /// At the rate `density` asks for, regularly or at random.
    Density,
    /// Only where a clock division falls: `true` on this frame.
    Clock(bool),
}

#[derive(Clone, Copy, Default)]
struct Grain {
    active: bool,
    index: usize,
    fraction: f32,
    ratio: f32,
    /// Through the window, 0 to 2 (rising to 1, falling to 2).
    phase: f32,
    phase_step: f32,
    /// Above 0: how far the triangle is bent towards a bell.
    smoothness: f32,
    /// With no smoothness: how steeply the window's edges rise.
    slope: f32,
    gain_left: f32,
    gain_right: f32,
    /// 1 while the grain plays out, falling to 0 if it must end early.
    release: f32,
    release_step: f32,
}

impl Grain {
    #[inline]
    fn envelope(&self) -> f32 {
        let triangle = if self.phase >= 1.0 {
            2.0 - self.phase
        } else {
            self.phase
        };
        if self.smoothness > 0.0 || self.slope <= 0.0 {
            // A raised-cosine bell, 0 to 1 as the triangle goes.
            let bell = 0.5 - 0.5 * cosine(0.5 * triangle);
            triangle + self.smoothness * (bell - triangle)
        } else {
            (triangle * self.slope).min(1.0)
        }
    }
}

/// The grain pool and the scheduler that starts its grains.
pub struct GrainCloud {
    grains: [Grain; MAX_GRAINS],
    rng: Rng,
    shortest: f32,
    /// Frames since the last grain started.
    since_grain: f32,
    /// Recent grains' length, in frames, which sets the grain rate.
    size_hint: f32,
    /// How many grains are playing, smoothed.
    playing: f32,
    rise: f32,
    fall: f32,
    normalization: f32,
    normalization_step: f32,
    release_frames: f32,
    shortest_release_frames: f32,
}

/// The share a one-pole moved per 32-frame block at 32 kHz becomes per
/// frame at `sample_rate`, so its time constant is unchanged.
pub fn share_per_frame(share_per_block: f32, sample_rate: f32) -> f32 {
    let frames_per_block = 32.0 * sample_rate / 32_000.0;
    1.0 - (1.0 - share_per_block).powf(1.0 / frames_per_block)
}

impl GrainCloud {
    pub fn new(sample_rate: u32, seed: u64) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            grains: [Grain::default(); MAX_GRAINS],
            rng: Rng::new(seed),
            shortest: SHORTEST_GRAIN * rate,
            since_grain: 0.0,
            size_hint: SHORTEST_GRAIN * rate,
            playing: 0.0,
            rise: share_per_frame(0.9, rate),
            fall: share_per_frame(0.2, rate),
            normalization: 1.0,
            normalization_step: 1.0 - 0.99f32.powf(32_000.0 / rate),
            release_frames: (RELEASE_SECONDS * rate).max(1.0),
            shortest_release_frames: (SHORTEST_RELEASE_SECONDS * rate).max(1.0),
        }
    }

    /// One frame of grains, starting one if `onsets` (or `trigger`) says
    /// so and a grain is free. `overlap` is how much grains overlap, 0 to
    /// 1; `regular` spaces them evenly rather than at random.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        recording: &Recording,
        shape: &GrainShape,
        overlap: f32,
        regular: bool,
        onsets: Onsets,
        trigger: bool,
    ) -> (f32, f32) {
        let wanted = MAX_GRAINS as f32 * overlap * overlap * overlap;
        self.since_grain += 1.0;
        let due = match onsets {
            Onsets::Clock(onset) => onset && wanted > 0.0,
            Onsets::Density if regular => {
                wanted > 0.0 && self.since_grain >= self.size_hint / wanted
            }
            Onsets::Density => self.rng.unit() < wanted / self.size_hint && wanted > self.playing,
        };
        if due || trigger {
            if let Some(free) = self.grains.iter().position(|grain| !grain.active) {
                self.start(free, recording, shape);
                self.since_grain = 0.0;
            }
        }

        let (mut left, mut right) = (0.0, 0.0);
        let mut playing = 0usize;
        for grain in self.grains.iter_mut().filter(|grain| grain.active) {
            let envelope = grain.envelope() * grain.release;
            let (l, r) = recording.read(grain.index, grain.fraction);
            let part_left = (l * grain.gain_left + r * (1.0 - grain.gain_right)) * envelope;
            let part_right = (r * grain.gain_right + l * (1.0 - grain.gain_left)) * envelope;
            left += part_left;
            right += part_right;
            grain.fraction += grain.ratio;
            let whole = grain.fraction.floor();
            grain.fraction -= whole;
            grain.index = recording.wrap((grain.index + whole as usize) as isize);
            grain.phase += grain.phase_step;
            grain.release -= grain.release_step;
            grain.active = grain.phase < 2.0 && grain.release > 0.0;
            playing += 1;
        }

        // Many overlapping grains add up: scale by about one over the root
        // of how many play, as the original does, and lift a bell window's
        // lower average level. That assumes the grains add up in power.
        // Grains reading the same audio add up in amplitude instead, so a
        // dense unpitched cloud comes out louder and leans into the output's
        // soft limit: the grit is kept on purpose.
        let playing = playing as f32;
        let share = if playing > self.playing {
            self.rise
        } else {
            self.fall
        };
        self.playing += share * (playing - self.playing);
        if self.playing < SILENT {
            self.playing = 0.0;
        }
        let mut target = if self.playing > 2.0 {
            (self.playing - 1.0).sqrt().recip()
        } else {
            1.0
        };
        let window_gain = (1.0 + 2.0 * shape.window).clamp(1.0, 2.0);
        target *= 1.0 + (window_gain - 1.0) * overlap;
        self.normalization += self.normalization_step * (target - self.normalization);
        (left * self.normalization, right * self.normalization)
    }

    /// Stops every grain, so the cloud starts afresh when next played.
    pub fn clear(&mut self) {
        for grain in &mut self.grains {
            grain.active = false;
        }
        self.since_grain = 0.0;
    }

    /// Recording starts again after a freeze. A grain read slower than the
    /// recording runs, which crossed the still write head while frozen, now
    /// sits just ahead of it: if the head would overrun it before it ends,
    /// release it before the head gets there.
    pub fn thaw(&mut self, recording: &Recording) {
        let (head, length) = (recording.head(), recording.len());
        for grain in self.grains.iter_mut().filter(|grain| grain.active) {
            let closing = 1.0 - grain.ratio;
            if closing <= 0.0 {
                continue;
            }
            let ahead = ((grain.index + length - head) % length) as f32 - READ_REACH;
            let remaining = (2.0 - grain.phase) / grain.phase_step;
            if ahead > closing * remaining {
                continue;
            }
            // Overrun within the write fade-in, a short release is quieter
            // than a cut.
            let frames = (ahead.max(0.0) / closing)
                .min(self.release_frames)
                .max(self.shortest_release_frames);
            grain.release_step = grain.release.max(0.0) / frames;
        }
    }

    fn start(&mut self, slot: usize, recording: &Recording, shape: &GrainShape) {
        let length = recording.len() as f32;
        let ratio = shape.ratio;
        // A grain read at `ratio` drifts from the write head by `1 - ratio`
        // frames a frame: its delay must stay inside the buffer, clear of
        // the write head, for the whole grain. At unity it never drifts, so
        // a grain may be longer than the buffer.
        let drift = (1.0 - ratio).abs();
        let span = length - 2.0 * MARGIN - 1.0;
        let mut size = self.shortest * (4.0 * shape.size).exp2();
        if drift > 0.0 {
            size = size.min(span / drift);
        }
        let size = size.max(2.0);
        // The nearest delay the grain reaches, set by `position`, and the
        // delay it starts from.
        let nearest = MARGIN + shape.position * (span - drift * size).max(0.0);
        let back = if ratio > 1.0 {
            nearest + (ratio - 1.0) * size
        } else {
            nearest
        };
        let start = recording.head() as f64 - f64::from(back);
        let start = if start < 0.0 {
            start + f64::from(length)
        } else {
            start
        };
        let width = (size as usize & !1).max(2);

        let pan = 0.5 + shape.spread * (self.rng.unit() - 0.5);
        let (gain_left, gain_right) = if pan < 0.5 {
            (1.0, 2.0 * pan)
        } else {
            (2.0 * (1.0 - pan), 1.0)
        };
        let window = shape.window;
        let (smoothness, slope) = if window >= 0.5 {
            ((window - 0.5) * 2.0, 0.0)
        } else {
            (0.0, 0.5 / (window + 0.01))
        };
        let whole = start.floor();
        self.grains[slot] = Grain {
            active: true,
            index: (whole as usize).min(recording.len() - 1),
            fraction: (start - whole) as f32,
            ratio,
            phase: 0.0,
            phase_step: 2.0 / width as f32,
            smoothness,
            slope,
            gain_left,
            gain_right,
            release: 1.0,
            release_step: 0.0,
        };
        self.size_hint += 0.1 * (size - self.size_hint);
    }
}
