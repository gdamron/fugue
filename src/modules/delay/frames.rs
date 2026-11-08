//! The delay frame by frame, whole or split around its line's writes.
//!
//! Each frame reads the line, advances the glides and the hold loop, and
//! asks the line to record its input plus the feedback. Only that write
//! needs the input, and what a frame reads is at least the shortest time
//! back, so a loop patched around the delay can run a chunk that long at a
//! time (see [`crate::FeedbackCut`]): [`Delay::emit_frames`] plays the chunk and
//! keeps its writes, the loop's other modules run, then
//! [`Delay::absorb_frames`] records them with the input they made. Both run the
//! same frames as [`Delay::run`] and read the same samples, so either
//! way the output is the same bit for bit.

use super::{soft_clip, Delay};

/// A frame's write to the line: the input is added to `fed` (the feedback
/// share of what the frame read), clipped, and recorded at `level`.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Write {
    fed: f32,
    level: f32,
}

impl Write {
    /// What the line records for `input`.
    #[inline]
    fn of(self, input: f32) -> f32 {
        let input = if input.is_finite() { input } else { 0.0 };
        soft_clip(input + self.fed) * self.level
    }
}

impl Delay {
    /// Prepares a run of frames: the time the controls and tempo ask for
    /// now, and, before the first frame is played, no glide to it.
    fn start(&mut self, frames: usize) {
        self.heads.retarget(self.time_samples());
        if !self.heard && frames > 0 {
            // The first frame plays where the controls and tempo are now.
            self.heads.settle();
            self.feedback.settle();
            self.heard = true;
        }
    }

    /// Frame `i`'s reads, `behind` writes before the line has them: what
    /// the delay plays before hold, and the write it asks for.
    #[inline]
    fn frame(&mut self, i: usize, modulation: f32, behind: usize) -> (f32, Option<Write>) {
        let reads = self.heads.next();
        let shift = self.inputs.time_mod[i] * modulation;
        let shift = if shift.is_finite() { shift } else { 0.0 };
        let mut live = self.read(reads.from, shift, behind);
        if reads.weight > 0.0 {
            let incoming = self.read(reads.to, shift, behind);
            live += reads.weight * (incoming - live);
        }
        let feedback = self.feedback.next_value();
        let write = self.hold.recording().then(|| {
            let level = self.hold.write_level();
            self.hold.recorded();
            Write {
                fed: feedback * live,
                level,
            }
        });
        (live, write)
    }

    /// Plays and records `frames` frames.
    pub(super) fn run(&mut self, frames: usize) {
        self.start(frames);
        let modulation = self.time_mod_depth * self.sample_rate as f32;
        for i in 0..frames {
            let (live, write) = self.frame(i, modulation, 0);
            if let Some(write) = write {
                self.line.write_and_advance(write.of(self.inputs.audio[i]));
            }
            self.out[i] = self.hold.next(live, &self.line, 0);
        }
    }

    /// Plays `frames` frames (at most the shortest time) without the
    /// `audio` input, keeping their writes for [`Self::absorb_frames`].
    pub(super) fn emit_frames(&mut self, frames: usize) {
        let frames = frames.min(self.shortest).min(self.writes.len());
        self.start(frames);
        let modulation = self.time_mod_depth * self.sample_rate as f32;
        let mut behind = 0;
        for i in 0..frames {
            let (live, write) = self.frame(i, modulation, behind);
            behind += usize::from(write.is_some());
            self.writes[i] = write;
            self.out[i] = self.hold.next(live, &self.line, behind);
        }
        self.emitted = frames;
    }

    /// Records the writes [`Self::emit_frames`] kept, with the `audio`
    /// input of the same frames.
    pub(super) fn absorb_frames(&mut self, frames: usize) {
        let frames = frames.min(self.emitted);
        for i in 0..frames {
            if let Some(write) = self.writes[i] {
                self.line.write_and_advance(write.of(self.inputs.audio[i]));
            }
        }
        self.emitted = 0;
    }
}
