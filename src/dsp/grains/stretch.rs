//! Time-stretching playback: two overlapping windows read the recording,
//! each new one placed where it best matches what the last is playing, so
//! the sound holds still in time (or crawls) while its pitch moves freely.
//!
//! This is WSOLA (waveform-similarity overlap-add), as in Mutable
//! Instruments' `clouds/dsp/wsola_sample_player.h` (MIT, Copyright 2014
//! Emilie Gillet): a window starts every half window, read at the pitch
//! ratio from a point set by `position`, nudged to line up with the window
//! already playing. Fugue's version is rewritten rather than ported: the
//! match is a normalised correlation over float samples, worked out a few
//! candidates a frame across the half window before it is needed (so no
//! frame does it all), and the windows are kept clear of the write head
//! the way grains are. See THIRD_PARTY_NOTICES.md.

use super::Recording;
use crate::dsp::cosine;

/// The shortest window (`size` 0), in seconds; `size` 1 is 4 times longer.
const SHORTEST_WINDOW: f32 = 0.032;
/// Frames kept between a window's reads and the write head.
const MARGIN: f64 = 4.0;
/// Output frames between the samples a match compares.
const COMPARE_STRIDE: usize = 4;

/// What the next windows are made of.
#[derive(Clone, Copy, Default)]
pub struct StretchShape {
    /// 0 reads the newest audio, 1 the oldest.
    pub position: f32,
    /// 0 to 1, a window of 32 ms up to 128 ms.
    pub size: f32,
    /// Playback rate (2 an octave up).
    pub ratio: f32,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    /// Where it reads, in frames from the buffer's start.
    at: f64,
    ratio: f64,
    /// Through the window, 0 to 1.
    phase: f32,
    phase_step: f32,
}

/// The search for where the next window starts.
#[derive(Clone, Copy, Default)]
struct Search {
    /// Where the playing window will be when the next one starts, and
    /// where the next would start unaligned.
    from: f64,
    target: f64,
    /// Source frames between compared samples, and how many are compared.
    stride: f64,
    points: usize,
    /// Candidate offsets from `target`, every other frame within `span`.
    span: i32,
    next: i32,
    per_frame: usize,
    best: i32,
    best_score: f32,
}

/// Two-window WSOLA playback over a [`Recording`].
pub struct Stretcher {
    voices: [Voice; 2],
    search: Search,
    shortest: f32,
    /// Frames until the next window starts.
    until_next: usize,
    /// Start the next window from the newest audio.
    restart: bool,
}

impl Stretcher {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            voices: [Voice::default(); 2],
            search: Search::default(),
            shortest: SHORTEST_WINDOW * sample_rate.max(1) as f32,
            until_next: 0,
            restart: false,
        }
    }

    /// Stops playing, so the next frame starts afresh.
    pub fn clear(&mut self) {
        self.voices = [Voice::default(); 2];
        self.until_next = 0;
    }

    /// Starts the next window from the newest audio, wherever `position`
    /// points.
    pub fn restart(&mut self) {
        self.restart = true;
    }

    /// One frame. `recording` says whether the write head is moving, so
    /// the next window can be placed where the audio will be.
    pub fn tick(
        &mut self,
        buffer: &Recording,
        shape: &StretchShape,
        recording: bool,
    ) -> (f32, f32) {
        if self.until_next == 0 {
            self.switch(buffer, shape, recording);
        }
        self.until_next -= 1;
        self.look(buffer);

        let length = buffer.len() as f64;
        let (mut left, mut right) = (0.0, 0.0);
        for voice in self.voices.iter_mut().filter(|voice| voice.active) {
            let gain = 0.5 - 0.5 * cosine(voice.phase);
            let whole = voice.at.floor();
            let (l, r) = buffer.read(whole as usize, (voice.at - whole) as f32);
            left += l * gain;
            right += r * gain;
            voice.at += voice.ratio;
            if voice.at >= length {
                voice.at -= length;
            }
            voice.phase += voice.phase_step;
            voice.active = voice.phase < 1.0;
        }
        (left, right)
    }

    /// Starts a window where the search found the best match, and sets up
    /// the search for the one after.
    fn switch(&mut self, buffer: &Recording, shape: &StretchShape, recording: bool) {
        let length = buffer.len() as f64;
        let ratio = f64::from(shape.ratio.max(0.0));
        let window = ((self.shortest * (2.0 * shape.size).exp2()) as usize)
            .min(buffer.len() / 8)
            .max(16)
            & !1;
        let hop = window / 2;
        let span = (hop / 2) as i32;
        let points = (hop / 2 / COMPARE_STRIDE).max(1);
        let stride = COMPARE_STRIDE as f64 * ratio;

        let start = if self.restart || !self.voices.iter().any(|voice| voice.active) {
            self.restart = false;
            let delay = self.delay(length, window, span, points as f64 * stride, ratio, 0.0);
            buffer.head() as f64 - delay
        } else {
            self.search.target + f64::from(self.search.best)
        };
        let start = start.rem_euclid(length);
        if let Some(voice) = self.voices.iter_mut().find(|voice| !voice.active) {
            *voice = Voice {
                active: true,
                at: start,
                ratio,
                phase: 0.0,
                phase_step: 1.0 / window as f32,
            };
        }
        self.until_next = hop;

        // The next window starts a hop from now, where the head will be.
        let head = buffer.head() as f64 + if recording { hop as f64 } else { 0.0 };
        let delay = self.delay(
            length,
            window,
            span,
            points as f64 * stride,
            ratio,
            shape.position,
        );
        let candidates = span as usize + 1;
        self.search = Search {
            from: (start + hop as f64 * ratio).rem_euclid(length),
            target: (head - delay).rem_euclid(length),
            stride,
            points,
            span,
            next: -span,
            per_frame: candidates.div_ceil(hop.saturating_sub(1).max(1)),
            best: 0,
            best_score: f32::NEG_INFINITY,
        };
    }

    /// How far behind the write head a window starts, for `position`: far
    /// enough that what the search compares has been recorded and the
    /// window never reaches the head, near enough that it never reaches the
    /// audio being overwritten.
    fn delay(
        &self,
        length: f64,
        window: usize,
        span: i32,
        compared: f64,
        ratio: f64,
        position: f32,
    ) -> f64 {
        let (window, span, hop) = (window as f64, f64::from(span), (window / 2) as f64);
        let nearest = MARGIN + span + hop + compared + ((ratio - 1.0) * window).max(0.0);
        let farthest = length - MARGIN - span - ((1.0 - ratio) * window).max(0.0);
        nearest + f64::from(position.clamp(0.0, 1.0)) * (farthest - nearest).max(0.0)
    }

    /// Scores a few more candidates for the next window's start.
    fn look(&mut self, buffer: &Recording) {
        for _ in 0..self.search.per_frame {
            if self.search.next > self.search.span {
                return;
            }
            let offset = self.search.next;
            self.search.next += 2;
            let score = self.score(buffer, offset);
            if score > self.search.best_score {
                self.search.best_score = score;
                self.search.best = offset;
            }
        }
    }

    /// How alike what the playing window will read and what a window
    /// started at `offset` would, as a correlation normalised by the
    /// candidate's level (so a louder stretch doesn't win for being loud).
    fn score(&self, buffer: &Recording, offset: i32) -> f32 {
        let search = &self.search;
        let mono = |at: f64| {
            let (l, r) = buffer.read(buffer.wrap(at.round() as isize), 0.0);
            l + r
        };
        let (mut product, mut energy) = (0.0, 0.0);
        let candidate = search.target + f64::from(offset);
        for k in 0..search.points {
            let step = k as f64 * search.stride;
            let x = mono(candidate + step);
            product += mono(search.from + step) * x;
            energy += x * x;
        }
        product / (energy + 1e-9).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn filled(frames: usize, hz: f32) -> Recording {
        let mut buffer = Recording::new(SR, frames);
        for n in 0..frames * 2 {
            let x = (std::f32::consts::TAU * hz * n as f32 / SR as f32).sin();
            buffer.write(x, x, true);
        }
        buffer
    }

    fn largest_step(x: &[f32]) -> f32 {
        x.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn overlapping_windows_hold_a_frozen_tone_steady() {
        let buffer = filled(SR as usize, 220.0);
        let mut stretcher = Stretcher::new(SR);
        let shape = StretchShape {
            position: 0.5,
            size: 0.5,
            ratio: 1.0,
        };
        let out: Vec<f32> = (0..SR as usize)
            .map(|_| stretcher.tick(&buffer, &shape, false).0)
            .collect();
        let settled = &out[SR as usize / 4..];
        let peak = settled.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((0.9..1.1).contains(&peak), "a peak of {peak}");
        // The windows line up: no step beyond the tone's own.
        let tone = std::f32::consts::TAU * 220.0 / SR as f32;
        assert!(
            largest_step(settled) <= 1.5 * tone,
            "{}",
            largest_step(settled)
        );
    }

    #[test]
    fn a_ratio_of_two_plays_an_octave_up() {
        let buffer = filled(SR as usize, 200.0);
        let mut stretcher = Stretcher::new(SR);
        let shape = StretchShape {
            position: 0.3,
            size: 1.0,
            ratio: 2.0,
        };
        let out: Vec<f32> = (0..SR as usize)
            .map(|_| stretcher.tick(&buffer, &shape, false).0)
            .collect();
        let rises = out[SR as usize / 2..]
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        // Half a second of 400 Hz.
        assert!((195..=205).contains(&rises), "{rises} cycles");
    }
}
