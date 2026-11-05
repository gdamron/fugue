//! A delay that becomes a loop: while recording, one reader trails the
//! write head by the delay; frozen, it loops the audio behind the head.
//!
//! As in Mutable Instruments' `clouds/dsp/looping_sample_player.h` (MIT,
//! Copyright 2014 Emilie Gillet): `position` sets the delay, or two taps
//! on the trigger (or a synced clock) set it to the time between them;
//! frozen, `size` (or that tapped time) sets the loop's length. Fugue's
//! version differs in how it moves: every jump of the reader (a new delay,
//! the loop coming round, a freeze or unfreeze) is a short crossfade from
//! where it was reading, so nothing steps, and the loop is kept wholly
//! behind the still head. See THIRD_PARTY_NOTICES.md.

use super::Recording;
use crate::dsp::cosine;

/// Frames kept between the reader and the write head, beyond the
/// interpolation's own reach.
const MARGIN: f64 = 8.0;
/// How long a jump crossfades.
const JUMP_SECONDS: f32 = 0.01;
/// The shortest loop, in seconds (`size` 0).
const SHORTEST_LOOP: f32 = 0.02;

/// What the reader follows.
#[derive(Clone, Copy, Default)]
pub struct LoopShape {
    /// 0 the shortest delay, 1 the longest the buffer holds.
    pub position: f32,
    /// The loop's length while frozen, 0 the shortest, 1 the longest.
    pub size: f32,
    /// A clock's period in frames, which then sets the delay and loop.
    pub period: Option<u32>,
}

/// One reader over a [`Recording`], with a crossfade for its jumps.
pub struct Looper {
    active: bool,
    at: f64,
    /// Where the reader was before its last jump, while that fades out;
    /// `None` fades in from silence.
    old: Option<f64>,
    fade: u32,
    fade_frames: u32,
    /// While frozen, frames left before the loop comes round.
    cycle: Option<f64>,
    /// Frames since the last tap, and the time between the last two.
    since_tap: Option<u32>,
    tapped: Option<u32>,
    shortest_loop: f64,
}

impl Looper {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        Self {
            active: false,
            at: 0.0,
            old: None,
            fade: 0,
            fade_frames: (JUMP_SECONDS * rate).round().max(1.0) as u32,
            cycle: None,
            since_tap: None,
            tapped: None,
            shortest_loop: f64::from(SHORTEST_LOOP * rate),
        }
    }

    /// Stops reading, so the next frame fades in afresh.
    pub fn clear(&mut self) {
        self.active = false;
        self.cycle = None;
        self.fade = 0;
    }

    /// A tap: two set the delay (and the loop) to the time between them.
    pub fn tap(&mut self) {
        if let Some(since) = self.since_tap {
            self.tapped = Some(since.max(1));
        }
        self.since_tap = Some(0);
    }

    /// Lets `position` set the delay again, after taps.
    pub fn forget_taps(&mut self) {
        self.tapped = None;
        self.since_tap = None;
    }

    /// One frame, read after this frame's write. `recording` says whether
    /// the write head is moving.
    pub fn tick(&mut self, buffer: &Recording, shape: &LoopShape, recording: bool) -> (f32, f32) {
        if let Some(since) = self.since_tap.as_mut() {
            *since = since.saturating_add(1);
        }
        let length = buffer.len() as f64;
        let head = buffer.head() as f64;
        let jump = f64::from(self.fade_frames);
        let longest = (length - 2.0 * MARGIN - jump).max(1.0);
        let set = shape.period.or(self.tapped).map(f64::from);
        let delay = set
            .unwrap_or_else(|| f64::from(shape.position.clamp(0.0, 1.0)) * longest)
            .round()
            .clamp(MARGIN, longest);

        if recording {
            self.cycle = None;
            self.follow(place(head - 1.0 - delay, length), length);
        } else {
            // The loop is set as it comes round, so `size` and `position`
            // move it a loop at a time.
            let due = match self.cycle {
                None => true,
                Some(left) => left <= 0.0 && self.fade == 0,
            };
            if due {
                let span = set.unwrap_or_else(|| {
                    let size = f64::from(shape.size.clamp(0.0, 1.0));
                    self.shortest_loop * (longest / self.shortest_loop).max(1.0).powf(size)
                });
                let span = span
                    .round()
                    .clamp(2.0 * jump, (longest - MARGIN).max(2.0 * jump));
                // The loop and the fade past its end stay behind the head.
                let start = place(head - delay.max(span + MARGIN + jump), length);
                if self.cycle.is_none() {
                    self.follow(start, length);
                } else {
                    self.jump(start);
                }
                self.cycle = Some(span);
            }
        }
        self.read(buffer, length)
    }

    /// Reads at `to` from now: at once, if already there; else crossfading.
    fn follow(&mut self, to: f64, length: f64) {
        if !self.active {
            self.active = true;
            self.at = to;
            self.old = None;
            self.fade = self.fade_frames;
            return;
        }
        let apart = (self.at - to).rem_euclid(length);
        if apart.min(length - apart) > 0.5 && self.fade == 0 {
            self.jump(to);
        }
    }

    fn jump(&mut self, to: f64) {
        self.old = Some(self.at);
        self.at = to;
        self.fade = self.fade_frames;
    }

    fn read(&mut self, buffer: &Recording, length: f64) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        let read = |at: f64| {
            let whole = at.floor();
            buffer.read(whole as usize, (at - whole) as f32)
        };
        let (mut left, mut right) = read(self.at);
        if self.fade > 0 {
            let x = 1.0 - self.fade as f32 / self.fade_frames as f32;
            let gain = 0.5 - 0.5 * cosine(0.5 * x);
            let (old_left, old_right) = self.old.map_or((0.0, 0.0), read);
            left = old_left + gain * (left - old_left);
            right = old_right + gain * (right - old_right);
            self.fade -= 1;
            if let Some(old) = self.old.as_mut() {
                *old = advance(*old, length);
            }
        }
        self.at = advance(self.at, length);
        if let Some(left) = self.cycle.as_mut() {
            *left -= 1.0;
        }
        (left, right)
    }
}

/// `at` wrapped into the buffer (`rem_euclid` alone can round up to the
/// length itself).
fn place(at: f64, length: f64) -> f64 {
    let at = at.rem_euclid(length);
    if at >= length {
        at - length
    } else {
        at
    }
}

fn advance(at: f64, length: f64) -> f64 {
    let at = at + 1.0;
    if at >= length {
        at - length
    } else {
        at
    }
}

#[cfg(test)]
#[path = "looper_tests.rs"]
mod tests;
