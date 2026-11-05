//! Time-stretching playback: overlapping windows read the recording, each
//! new one placed where it best matches what the last is playing, so the
//! sound holds still in time (or crawls) while its pitch moves freely.
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
//!
//! Each window rises over the hop it starts on and falls over the next, as
//! the next rises: the two always sum to one, even as `size` changes the
//! hop from one window to the next.

use super::Recording;
use crate::dsp::cosine;

/// The shortest window (`size` 0), in seconds; `size` 1 is 4 times longer.
const SHORTEST_WINDOW: f32 = 0.032;
/// Frames kept between a window's reads and the write head, beyond the
/// interpolation's own reach.
const MARGIN: f64 = 7.0;
/// Output frames between the samples a match compares.
const COMPARE_STRIDE: usize = 4;
/// The quick crossfade a trigger or an unfreeze cuts to a new window with.
const CUT_SECONDS: f32 = 0.005;
const SHORTEST_CUT_SECONDS: f32 = 0.001;
/// Windows: two overlapping, and a third for a cut.
const VOICES: usize = 3;

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
    /// A raised-cosine fade from one gain to another over `frames`.
    from: f32,
    to: f32,
    frame: u32,
    frames: u32,
    per_frame: f32,
}

impl Voice {
    fn gain(&self) -> f32 {
        let x = self.frame as f32 * self.per_frame;
        self.from + (self.to - self.from) * (0.5 - 0.5 * cosine(0.5 * x))
    }

    fn fade(&mut self, to: f32, frames: u32) {
        self.from = self.gain();
        self.to = to;
        self.frame = 0;
        self.frames = frames.max(1);
        self.per_frame = 1.0 / self.frames as f32;
    }

    fn fading_out(&self) -> bool {
        self.active && self.to == 0.0
    }
}

/// The search for where the next window starts.
#[derive(Clone, Copy, Default)]
struct Search {
    /// Where the next would start unaligned.
    target: f64,
    /// Source frames between compared samples, and how many are compared.
    stride: f64,
    points: usize,
    /// Candidate offsets from `target`, every other frame within `span`,
    /// then a frame either side of the best of those.
    span: i32,
    coarse: usize,
    next: usize,
    per_frame: usize,
    best: i32,
    best_score: f32,
}

/// WSOLA playback over a [`Recording`].
pub struct Stretcher {
    voices: [Voice; VOICES],
    search: Search,
    /// What the playing window will read when the next starts, as the
    /// search compares it.
    from: Vec<f32>,
    shortest: f32,
    shape: StretchShape,
    hop: usize,
    /// Frames until the next window starts.
    until_next: usize,
    /// A cut asked for, over this many frames, and the frames left of
    /// the one under way.
    cut: Option<u32>,
    cutting: u32,
    cut_frames: f32,
    shortest_cut_frames: f32,
}

impl Stretcher {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let shortest = SHORTEST_WINDOW * rate;
        // The most points compared: a quarter hop of the longest window.
        let most_points = (4.0 * shortest) as usize / 2 / 2 / COMPARE_STRIDE + 2;
        Self {
            voices: [Voice::default(); VOICES],
            search: Search::default(),
            from: vec![0.0; most_points],
            shortest,
            shape: StretchShape::default(),
            hop: 0,
            until_next: 0,
            cut: None,
            cutting: 0,
            cut_frames: (CUT_SECONDS * rate).max(1.0),
            shortest_cut_frames: (SHORTEST_CUT_SECONDS * rate).max(1.0),
        }
    }

    /// Stops playing, so the next frame starts afresh.
    pub fn clear(&mut self) {
        self.voices = [Voice::default(); VOICES];
        self.until_next = 0;
        self.cut = None;
        self.cutting = 0;
    }

    /// Cuts quickly to a window at `position`, without lining it up: from
    /// there, playback stretches on as before.
    pub fn restart(&mut self) {
        self.ask_cut(self.cut_frames as u32);
    }

    /// Recording restarts after a freeze. A window reading close ahead of
    /// the still head (below unity) or close behind it (above) would be
    /// overrun once it moves: cut to a fresh window before that happens.
    pub fn thaw(&mut self, buffer: &Recording) {
        let (head, length) = (buffer.head() as f64, buffer.len() as f64);
        let later = self.until_next as f64 + self.longest_hop(buffer) as f64;
        let mut soonest = f64::INFINITY;
        for voice in self.voices.iter().filter(|voice| voice.active) {
            let remaining = if voice.fading_out() {
                f64::from(voice.frames - voice.frame)
            } else {
                later
            };
            let ahead = (voice.at - head).rem_euclid(length);
            // Frames until the head reaches the voice, or it the head.
            let (gap, closing) = if voice.ratio < 1.0 {
                (ahead, 1.0 - voice.ratio)
            } else {
                (length - ahead, voice.ratio - 1.0)
            };
            let frames = (gap - MARGIN).max(0.0) / closing;
            if closing > 0.0 && frames <= remaining {
                soonest = soonest.min(frames);
            }
        }
        if soonest.is_finite() {
            let frames = (soonest as f32)
                .min(self.cut_frames)
                .max(self.shortest_cut_frames);
            self.ask_cut(frames as u32);
        }
    }

    /// One frame. `recording` says whether the write head is moving, so
    /// windows can be kept clear of it.
    pub fn tick(
        &mut self,
        buffer: &Recording,
        shape: &StretchShape,
        recording: bool,
    ) -> (f32, f32) {
        self.shape = *shape;
        match self.cut {
            // One cut at a time.
            Some(frames) if self.cutting == 0 => {
                self.cut = None;
                self.cutting = frames;
                self.begin(buffer, None, frames, recording);
            }
            _ if self.until_next == 0 => self.switch(buffer, recording),
            _ => {}
        }
        self.until_next = self.until_next.saturating_sub(1);
        self.cutting = self.cutting.saturating_sub(1);
        self.look(buffer);

        let length = buffer.len() as f64;
        let (mut left, mut right) = (0.0, 0.0);
        for voice in self.voices.iter_mut().filter(|voice| voice.active) {
            let gain = voice.gain();
            let whole = voice.at.floor();
            let (l, r) = buffer.read(whole as usize, (voice.at - whole) as f32);
            left += l * gain;
            right += r * gain;
            voice.at += voice.ratio;
            if voice.at >= length {
                voice.at -= length;
            }
            if voice.frame < voice.frames {
                voice.frame += 1;
            }
            if voice.to == 0.0 && voice.frame == voice.frames {
                voice.active = false;
            }
        }
        (left, right)
    }

    fn ask_cut(&mut self, frames: u32) {
        let frames = frames.max(1);
        self.cut = Some(self.cut.map_or(frames, |asked| asked.min(frames)));
    }

    /// The hop of the longest window the buffer allows.
    fn longest_hop(&self, buffer: &Recording) -> usize {
        self.window(buffer, 1.0) / 2
    }

    /// A window's length for `size`: at most an eighth of the buffer, at
    /// least 16 frames (so a buffer under 128 frames leaves `position`
    /// little room), and even.
    fn window(&self, buffer: &Recording, size: f32) -> usize {
        ((self.shortest * (2.0 * size).exp2()) as usize)
            .min(buffer.len() / 8)
            .max(16)
            & !1
    }

    /// Starts the next window where the search found the best match.
    fn switch(&mut self, buffer: &Recording, recording: bool) {
        let hop = self.window(buffer, self.shape.size) / 2;
        if self.voices.iter().any(|voice| voice.active) {
            let start = self.search.target + f64::from(self.search.best);
            self.begin(buffer, Some(start), hop as u32, recording);
        } else {
            self.begin(buffer, None, hop as u32, recording);
        }
    }

    /// Fades every playing window out and a new one in over `fade` frames:
    /// at `start`, or unaligned at `position` when `None`. Then sets up the
    /// search for the window after.
    fn begin(&mut self, buffer: &Recording, start: Option<f64>, fade: u32, recording: bool) {
        let length = buffer.len() as f64;
        let head = buffer.head() as f64;
        let ratio = f64::from(self.shape.ratio.max(0.0));
        let hop = self.window(buffer, self.shape.size) / 2;
        // A window plays over its own hop and the next one's.
        let reach = (hop + self.longest_hop(buffer)) as f64;
        let start = match start {
            Some(start) if recording => {
                // Pitch or size may have moved since the start was picked:
                // keep the window clear of the head all the same.
                let behind = (head - start).rem_euclid(length);
                let (nearest, farthest) = self.bounds(length, reach, ratio);
                head - behind.clamp(nearest, farthest.max(nearest))
            }
            Some(start) => start,
            None => head - self.delay(length, hop, reach, ratio, self.shape.position),
        };
        let mut start = start.rem_euclid(length);
        if start >= length {
            start -= length;
        }

        for voice in self.voices.iter_mut().filter(|voice| voice.active) {
            voice.fade(0.0, fade);
        }
        let free = self.voices.iter().position(|voice| !voice.active);
        debug_assert!(free.is_some(), "a cut waits for the last to finish");
        let voice = &mut self.voices[free.unwrap_or(0)];
        *voice = Voice {
            active: true,
            at: start,
            ratio,
            ..Voice::default()
        };
        voice.fade(1.0, fade);
        self.hop = hop;
        self.until_next = hop;
        self.prepare(buffer, start, recording);
    }

    /// Sets up the search for the window a hop from now: what the playing
    /// window will read then, and where the next would start unaligned.
    fn prepare(&mut self, buffer: &Recording, start: f64, recording: bool) {
        let length = buffer.len() as f64;
        let ratio = f64::from(self.shape.ratio.max(0.0));
        let hop = self.hop;
        let span = (hop / 2) as i32;
        let points = (hop / 2 / COMPARE_STRIDE).clamp(1, self.from.len());
        let stride = COMPARE_STRIDE as f64 * ratio;
        let from = start + hop as f64 * ratio;
        for (k, sample) in self.from[..points].iter_mut().enumerate() {
            *sample = buffer.mono(index(buffer, from + k as f64 * stride));
        }
        let head = buffer.head() as f64 + if recording { hop as f64 } else { 0.0 };
        let reach = (hop + self.longest_hop(buffer)) as f64;
        let delay = self.delay(length, hop, reach, ratio, self.shape.position);
        let coarse = span as usize + 1;
        self.search = Search {
            target: (head - delay).rem_euclid(length),
            stride,
            points,
            span,
            coarse,
            next: 0,
            per_frame: (coarse + 2).div_ceil(hop.saturating_sub(1).max(1)),
            best: 0,
            best_score: f32::NEG_INFINITY,
        };
    }

    /// How near and how far behind the write head a window may start, if
    /// it is to stay clear of the head for `reach` frames at `ratio`.
    fn bounds(&self, length: f64, reach: f64, ratio: f64) -> (f64, f64) {
        let nearest = MARGIN + ((ratio - 1.0) * reach).max(0.0);
        let farthest = length - MARGIN - ((1.0 - ratio) * reach).max(0.0);
        (nearest, farthest)
    }

    /// How far behind the write head a window starts unaligned, for
    /// `position`: near enough that the search's candidates (within half a
    /// hop) stay inside the bounds, far enough that what it compares has
    /// been recorded by the time it looks, a hop before the window starts.
    fn delay(&self, length: f64, hop: usize, reach: f64, ratio: f64, position: f32) -> f64 {
        let (nearest, farthest) = self.bounds(length, reach, ratio);
        let span = (hop / 2) as f64;
        let compared = (hop / 2 / COMPARE_STRIDE).max(1) as f64 * COMPARE_STRIDE as f64 * ratio;
        let nearest = nearest + hop as f64 + span + compared;
        let farthest = farthest - span;
        nearest + f64::from(position.clamp(0.0, 1.0)) * (farthest - nearest).max(0.0)
    }

    /// Scores a few more candidates for the next window's start.
    fn look(&mut self, buffer: &Recording) {
        for _ in 0..self.search.per_frame {
            let search = &self.search;
            if search.next >= search.coarse + 2 {
                return;
            }
            let offset = if search.next < search.coarse {
                2 * search.next as i32 - search.span
            } else if search.next == search.coarse {
                search.best - 1
            } else {
                search.best + 1
            };
            self.search.next += 1;
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
        let (mut product, mut energy) = (0.0, 0.0);
        let candidate = search.target + f64::from(offset);
        for (k, from) in self.from[..search.points].iter().enumerate() {
            let x = buffer.mono(index(buffer, candidate + k as f64 * search.stride));
            product += from * x;
            energy += x * x;
        }
        product / (energy + 1e-9).sqrt()
    }
}

/// The nearest whole frame to `at`, wrapped into the buffer.
fn index(buffer: &Recording, at: f64) -> usize {
    (at.round() as isize).rem_euclid(buffer.len() as isize) as usize
}

#[cfg(test)]
#[path = "stretch_tests.rs"]
mod tests;
