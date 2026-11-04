//! Hold: freeze the most recent stretch of the line and loop it.
//!
//! Holding stops the line recording and loops the last `length` samples it
//! recorded (one division: a beat repeat, or a stutter). The loop reads
//! whole samples from a line that no longer changes, so every pass after
//! the first is the same bit for bit. Its seam is crossfaded over a few
//! milliseconds into the samples that came just before the slice, which
//! are what originally led into its first sample, so the wrap never steps.
//! Engaging and releasing fade between the loop and the delay's own output.
//!
//! The line stops recording only after its writes fade out, and they fade
//! back in on release, so the gap hold leaves in the line is a short dip,
//! never a step, when the delay's own read head later passes over it.

use crate::dsp::{DelayLine, LinearRamp};

/// How long entering or leaving hold fades, and how long the seam's
/// crossfade lasts.
const FADE_SECONDS: f32 = 0.005;

/// The loop, owned by the audio thread.
pub(super) struct Hold {
    /// Whether the loop is heard at all (holding, or fading out).
    engaged: bool,
    /// Whether hold is asked for.
    holding: bool,
    /// Whether the line records: until the loop is fully in and the writes
    /// have faded out, and again from the moment hold is released.
    recording: bool,
    /// The level of what the line records, faded out before it stops and
    /// in again when it starts.
    writing: LinearRamp,
    /// The loop's level against the delay's own output.
    mix: LinearRamp,
    /// The loop's length, and the seam crossfade's, in samples.
    length: usize,
    seam: usize,
    fade_frames: usize,
    /// Where in the loop the next sample is.
    position: usize,
    /// Samples recorded since the slice ended (during the fades), which
    /// move the slice that much further back in the line.
    since: usize,
    /// A loop length asked for while the last slice was still fading out
    /// and could not be kept: taken once that slice has faded away.
    waiting: Option<usize>,
}

impl Hold {
    pub(super) fn new(sample_rate: u32) -> Self {
        let fade_frames = ((FADE_SECONDS * sample_rate as f32).round() as usize).max(1);
        Self {
            engaged: false,
            holding: false,
            recording: true,
            writing: LinearRamp::new(1.0, fade_frames as u32),
            mix: LinearRamp::new(0.0, fade_frames as u32),
            length: 1,
            seam: 0,
            fade_frames,
            position: 0,
            since: 0,
            waiting: None,
        }
    }

    /// The samples a line needs beyond its longest delay so a loop of that
    /// length, its seam, and the fades either side of it all fit.
    pub(super) fn headroom(sample_rate: u32) -> usize {
        Self::headroom_frames(Self::new(sample_rate).fade_frames)
    }

    /// The headroom for fades of `fade` samples. The slice reaches its
    /// length plus the seam (at most a fade) and one more back, plus
    /// whatever has been recorded since: at most seven fades, as a slice is
    /// kept only while three more fades (in, the writes' fade out, and out
    /// again) still fit.
    fn headroom_frames(fade: usize) -> usize {
        8 * fade + 3
    }

    /// Whether the slice still sounding can be held again: whether the
    /// fades a fresh hold and its release record still keep it in the line.
    fn reusable(&self) -> bool {
        let fade = self.fade_frames;
        self.engaged && self.since + 3 * fade + self.seam + 1 <= Self::headroom_frames(fade)
    }

    /// Takes the last `length` samples of a line `line_len` long as the
    /// slice to loop, from its start.
    fn capture(&mut self, length: usize, line_len: usize) {
        let room = line_len
            .saturating_sub(Self::headroom_frames(self.fade_frames))
            .max(2);
        self.length = length.clamp(2, room);
        self.seam = self.fade_frames.min(self.length / 2);
        self.position = 0;
        self.since = 0;
        self.engaged = true;
        self.mix.set_target(1.0);
    }

    /// Whether the line should record this sample.
    #[inline]
    pub(super) fn recording(&self) -> bool {
        self.recording
    }

    /// The level to record this sample at, which stops the recording once
    /// it has faded to nothing while holding.
    #[inline]
    pub(super) fn write_level(&mut self) -> f32 {
        let level = self.writing.next_value();
        if self.holding && level <= 0.0 {
            self.recording = false;
        }
        level
    }

    /// Notes that the line recorded a sample.
    #[inline]
    pub(super) fn recorded(&mut self) {
        if self.engaged {
            self.since += 1;
        }
    }

    /// Starts holding the last `length` samples of `line`. A slice still
    /// fading out is held again where it is, so a quick release and re-hold
    /// doesn't jump; one that no longer fits finishes fading out first,
    /// and the fresh slice is taken then (within a fade), never cut in
    /// while the old one sounds.
    pub(super) fn engage(&mut self, length: usize, line: &DelayLine) {
        if self.holding {
            return;
        }
        self.holding = true;
        if self.reusable() {
            self.mix.set_target(1.0);
        } else if self.engaged {
            self.waiting = Some(length);
        } else {
            self.capture(length, line.len());
        }
    }

    /// Stops holding: the line records again, and the loop fades out.
    pub(super) fn release(&mut self) {
        if !self.holding {
            return;
        }
        self.holding = false;
        self.waiting = None;
        self.recording = true;
        self.writing.set_target(1.0);
        self.mix.set_target(0.0);
    }

    /// This sample's output: the delay's own `live` output, the loop, or a
    /// fade between them.
    #[inline]
    pub(super) fn next(&mut self, live: f32, line: &DelayLine) -> f32 {
        if !self.engaged {
            return live;
        }
        let mix = self.mix.next_value();
        let looped = self.looped(line);
        self.position += 1;
        if self.position >= self.length {
            self.position = 0;
        }
        if self.holding && mix >= 1.0 {
            self.writing.set_target(0.0);
        }
        if mix <= 0.0 {
            match self.waiting.take() {
                Some(length) => self.capture(length, line.len()),
                None if !self.holding => self.engaged = false,
                None => {}
            }
        }
        if mix >= 1.0 {
            looped
        } else {
            live + mix * (looped - live)
        }
    }

    /// The loop's sample at its position. Offsets count back from the
    /// line's newest sample: the slice's first sample is `length` back
    /// (plus whatever was recorded since), its last is 1 back.
    #[inline]
    fn looped(&self, line: &DelayLine) -> f32 {
        let (length, p) = (self.length, self.position);
        let sample = line.read(length - p + self.since);
        let into_seam = (p + self.seam + 1).saturating_sub(length);
        if into_seam == 0 {
            return sample;
        }
        // Across the seam, fade into the samples just before the slice:
        // the ones that led into its first sample.
        let before = line.read(2 * length - p + self.since);
        let weight = into_seam as f32 / (self.seam + 1) as f32;
        sample + weight * (before - sample)
    }
}
