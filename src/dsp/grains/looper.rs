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
//!
//! With a glide time set, a `position` moved while recording is followed
//! as tape follows a moved head: the delay slides to its new length, the
//! reader speeding up or slowing down (and the pitch bending) on the way.
//! Only a leap too far to slide in reasonable time still crossfades, as do
//! tapped and clocked delays and everything while frozen.

use super::Recording;
use crate::dsp::cosine;

/// Frames kept between the reader and the write head, beyond the
/// interpolation's own reach.
const MARGIN: f64 = 8.0;
/// How long a jump crossfades.
const JUMP_SECONDS: f32 = 0.01;
/// The shortest loop, in seconds (`size` 0).
const SHORTEST_LOOP: f32 = 0.02;
/// How fast a gliding delay may change, in frames per frame: the reader
/// runs between half speed and double, an octave either way.
const GLIDE_SLOWEST: f64 = 0.5;
const GLIDE_FASTEST: f64 = 2.0;
/// The furthest a glide goes, as a share of the longest delay, in seconds,
/// or as far as the slowest reader falls behind in the glide time,
/// whichever is least; a longer move crossfades instead.
const GLIDE_SHARE: f64 = 0.25;
const GLIDE_SECONDS: f64 = 1.0;
/// Each of the glide's two smoothing stages takes this share of the glide
/// time as its time constant, so a move is about nine-tenths done by then.
const GLIDE_STAGE: f64 = 0.25;

/// What the reader follows.
#[derive(Clone, Copy, Default)]
pub struct LoopShape {
    /// 0 the shortest delay, 1 the longest the buffer holds.
    pub position: f32,
    /// The loop's length while frozen, 0 the shortest, 1 the longest.
    pub size: f32,
    /// A clock's period in frames, which then sets the delay and loop.
    pub period: Option<u32>,
    /// Frames what is read is delayed by afterwards (a pitch shifter's),
    /// taken off the delay so a tapped or clocked time stays on time.
    pub latency: u32,
}

/// One reader over a [`Recording`], with a crossfade for its jumps.
pub struct Looper {
    active: bool,
    at: f64,
    /// Where the reader was before its last jump, while that fades out.
    old: Option<f64>,
    fade: u32,
    fade_frames: u32,
    /// While frozen, frames left before the loop comes round, and where
    /// the head was as the freeze began (it runs on through its fade).
    cycle: Option<f64>,
    anchor: Option<f64>,
    /// Frames since the last tap, and the time between the last two; a
    /// tap after longer than the longest delay starts afresh.
    since_tap: Option<u32>,
    tapped: Option<u32>,
    longest_tap: u32,
    shortest_loop: f64,
    rate: f64,
    /// Each smoothing stage's step toward its target per frame; 0 with
    /// glide off, when every move jumps.
    glide_step: f64,
    /// While gliding, the delay each smoothing stage has reached: the
    /// second is the one read at.
    glided: Option<(f64, f64)>,
    /// The furthest a glide goes from the glide time alone, in frames.
    glide_reach: f64,
    /// Recording restarted after a freeze, and the reader has yet to
    /// crossfade from the loop to the delay.
    thawing: bool,
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
            anchor: None,
            since_tap: None,
            tapped: None,
            longest_tap: u32::MAX,
            shortest_loop: f64::from(SHORTEST_LOOP * rate),
            rate: f64::from(rate),
            glide_step: 0.0,
            glided: None,
            glide_reach: 0.0,
            thawing: false,
        }
    }

    /// How long a moved delay takes to glide, in seconds; 0 jumps. Off
    /// mid-glide, the next frame crossfades to where the delay is due;
    /// changed mid-glide, the read speed steps to the new time's pace.
    pub fn set_glide(&mut self, seconds: f32) {
        let time = f64::from(seconds.max(0.0)) * self.rate;
        self.glide_reach = (1.0 - GLIDE_SLOWEST) * time;
        let frames = time * GLIDE_STAGE;
        self.glide_step = if frames > 0.0 {
            1.0 - (-1.0 / frames.max(1.0)).exp()
        } else {
            0.0
        };
    }

    /// Stops reading, so the next frame starts afresh.
    pub fn clear(&mut self) {
        self.active = false;
        self.cycle = None;
        self.anchor = None;
        self.fade = 0;
        self.since_tap = None;
        self.glided = None;
        self.thawing = false;
    }

    /// Whether it is reading (else the next frame starts afresh).
    pub fn active(&self) -> bool {
        self.active
    }

    /// Where the next frame will be read.
    pub fn at(&self) -> f64 {
        self.at
    }

    /// A tap: two set the delay (and the loop) to the time between them.
    pub fn tap(&mut self) {
        match self.since_tap {
            Some(since) if since <= self.longest_tap => self.tapped = Some(since.max(1)),
            _ => {}
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
        self.longest_tap = longest as u32;
        let set = shape.period.or(self.tapped).map(f64::from);
        let delay = set
            .unwrap_or_else(|| f64::from(shape.position.clamp(0.0, 1.0)) * longest)
            .round()
            .clamp(MARGIN, longest);

        if recording {
            if self.glide_step > 0.0 && self.cycle.is_some() {
                self.thawing = true;
            }
            self.cycle = None;
            self.anchor = None;
            let delay = (delay - f64::from(shape.latency)).max(MARGIN);
            if self.glide_step > 0.0 && set.is_none() {
                let reach = (GLIDE_SHARE * longest)
                    .min(GLIDE_SECONDS * self.rate)
                    .min(self.glide_reach);
                self.glide(head, delay, length, reach);
            } else {
                self.glided = None;
                self.thawing = false;
                self.follow(place(head - 1.0 - delay, length), length);
            }
        } else {
            self.glided = None;
            // The head runs on through its write fade after a freeze: the
            // loop is placed from where it stood, and kept clear of what
            // the fade writes over (just past it, the oldest audio).
            let head = *self.anchor.get_or_insert(head);
            let longest = (longest - buffer.fade_frames() as f64).max(1.0);
            let delay = (delay - f64::from(shape.latency)).max(MARGIN).min(longest);
            // The loop is set as it comes round, so `size` and `position`
            // move it a loop at a time; never mid-jump, so it never starts
            // from where the reader was before one.
            let due = self.fade == 0 && self.cycle.is_none_or(|left| left <= 0.0);
            if due {
                let span = set.unwrap_or_else(|| {
                    let size = f64::from(shape.size.clamp(0.0, 1.0));
                    self.shortest_loop * (longest / self.shortest_loop).max(1.0).powf(size)
                });
                let span = span.round().clamp(2.0 * jump, longest.max(2.0 * jump));
                // The loop and the fade past its end stay behind the head.
                // Where the reader is at the freeze, if the loop fits there.
                let start = place(head - 1.0 - delay.max(span + MARGIN + jump), length);
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

    /// Slides the delay toward `delay` through two smoothing stages, so
    /// the reader's speed changes smoothly; starts from the delay being
    /// read, and crossfades instead past `reach` or out of a freeze.
    fn glide(&mut self, head: f64, delay: f64, length: f64, reach: f64) {
        let (fast, slow) = match self.glided {
            Some(state) => state,
            // Already following the head: glide on from where it reads.
            None if self.active && !self.thawing => {
                let now = place(head - 1.0 - self.at, length);
                (now, now)
            }
            // Starting, or out of a freeze: crossfade to the delay first,
            // once any crossfade of the loop is done.
            None => {
                if !self.active || self.fade == 0 {
                    self.follow(place(head - 1.0 - delay, length), length);
                    self.glided = Some((delay, delay));
                    self.thawing = false;
                }
                return;
            }
        };
        if (delay - slow).abs() > reach {
            // Too far to glide: crossfade there, once any crossfade is
            // done, holding the delay where it is until then.
            if self.fade == 0 {
                self.glided = Some((delay, delay));
                self.jump(place(head - 1.0 - delay, length));
            } else {
                self.at = place(head - 1.0 - slow, length);
            }
            return;
        }
        let fast = fast + self.glide_step * (delay - fast);
        // The read speed is 1 less the delay's change per frame.
        let change =
            (self.glide_step * (fast - slow)).clamp(1.0 - GLIDE_FASTEST, 1.0 - GLIDE_SLOWEST);
        let slow = slow + change;
        self.glided = Some((fast, slow));
        self.at = place(head - 1.0 - slow, length);
    }

    /// Reads at `to` from now: at once, if already there; else crossfading.
    fn follow(&mut self, to: f64, length: f64) {
        if !self.active {
            self.active = true;
            self.at = to;
            self.old = None;
            // Nothing to fade from: a mode change fades the whole mode in.
            self.fade = 0;
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
