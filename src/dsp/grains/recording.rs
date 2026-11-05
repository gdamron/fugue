//! The recording buffer: a circular stereo buffer the input is written
//! into and every playback mode reads from.
//!
//! Ported from Mutable Instruments' `clouds/dsp/audio_buffer.h` (MIT,
//! Copyright 2014 Emilie Gillet), adapted to Fugue: `f32` samples instead
//! of 16- or 8-bit, a length set from config, and a write fade in place of
//! the original's tail crossfade. The interpolation is stmlib's
//! `InterpolateHermite` (`dsp/dsp.h`, MIT, Copyright 2012 Emilie Gillet).
//! See THIRD_PARTY_NOTICES.md.
//!
//! Freezing stops the write head. A head that stopped dead would leave the
//! newest audio butted against what the buffer held a full length earlier,
//! and anything reading across that seam clicks. Instead the written level
//! fades out over 20 ms before the head stops (and back in
//! when it starts again): each write blends the input into what the buffer
//! already holds, so the seam is a short crossfade.

/// How long the written level takes to fade out on freeze, and back in on
/// unfreeze.
const WRITE_FADE_SECONDS: f32 = 0.02;

fn fade_frames(sample_rate: u32) -> u32 {
    ((sample_rate as f32 * WRITE_FADE_SECONDS).round() as u32).max(1)
}

/// A circular stereo recording, allocated once at build.
pub struct Recording {
    left: Vec<f32>,
    right: Vec<f32>,
    /// Where the next frame is written.
    head: usize,
    /// The level new frames are written at, in steps of the fade: all of
    /// them recording, none frozen.
    level: u32,
    fade_frames: u32,
}

impl Recording {
    /// A silent buffer of `frames` frames (at least 4, so an interpolated
    /// read always has its neighbours).
    pub fn new(sample_rate: u32, frames: usize) -> Self {
        let frames = frames.max(4);
        Self {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            head: 0,
            level: fade_frames(sample_rate),
            fade_frames: fade_frames(sample_rate),
        }
    }

    /// The buffer's length in frames.
    #[inline]
    pub fn len(&self) -> usize {
        self.left.len()
    }

    /// Never: a recording holds at least 4 frames.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.left.is_empty()
    }

    /// Frames the head runs on after a freeze, fading its writes out.
    #[inline]
    pub fn fade_frames(&self) -> usize {
        self.fade_frames as usize
    }

    /// Where the next frame will be written; the newest frame is one before.
    #[inline]
    pub fn head(&self) -> usize {
        self.head
    }

    /// Writes one frame while `recording`; while not, fades the written
    /// level out and then holds the head where it is.
    #[inline]
    pub fn write(&mut self, left: f32, right: f32, recording: bool) {
        if recording {
            self.level = (self.level + 1).min(self.fade_frames);
        } else if self.level > 0 {
            self.level -= 1;
        } else {
            return;
        }
        let at = self.head;
        let level = self.level as f32 / self.fade_frames as f32;
        self.left[at] += (left - self.left[at]) * level;
        self.right[at] += (right - self.right[at]) * level;
        self.head = if at + 1 == self.len() { 0 } else { at + 1 };
    }

    /// `index` wrapped into the buffer, for any index within one length
    /// either side of it.
    #[inline]
    pub fn wrap(&self, index: isize) -> usize {
        let len = self.len() as isize;
        (if index < 0 {
            index + len
        } else if index >= len {
            index - len
        } else {
            index
        }) as usize
    }

    /// Both channels summed at a whole `index`, uninterpolated: cheap
    /// enough to compare many stretches of audio.
    #[inline]
    pub fn mono(&self, index: usize) -> f32 {
        self.left[index] + self.right[index]
    }

    /// Both channels at `index + fraction`, by 4-point Hermite
    /// interpolation, reading across the buffer's end seamlessly.
    #[inline]
    pub fn read(&self, index: usize, fraction: f32) -> (f32, f32) {
        let i = index as isize;
        let at = [self.wrap(i - 1), index, self.wrap(i + 1), self.wrap(i + 2)];
        (
            hermite(&self.left, at, fraction),
            hermite(&self.right, at, fraction),
        )
    }
}

/// The 4-point, third-order Hermite curve through `x[at[1]]` (fraction 0)
/// and `x[at[2]]` (fraction 1).
#[inline]
fn hermite(x: &[f32], at: [usize; 4], t: f32) -> f32 {
    let (xm1, x0, x1, x2) = (x[at[0]], x[at[1]], x[at[2]], x[at[3]]);
    let c = (x1 - xm1) * 0.5;
    let v = x0 - x1;
    let w = c + v;
    let a = w + v + (x2 - x0) * 0.5;
    let b = w + a;
    (((a * t) - b) * t + c) * t + x0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_across_the_end_is_continuous() {
        let mut buffer = Recording::new(48_000, 100);
        for n in 0..250 {
            let x = (n as f32 * 0.05).sin();
            buffer.write(x, -x, true);
        }
        // The head is at 50; frames 49 back to 50 (through 99 → 0) hold a
        // continuous sine.
        let mut last = buffer.read(60, 0.0).0;
        for step in 1..300 {
            let position = 60.0 + step as f32 * 0.25;
            let index = position.floor() as usize % 100;
            let (l, r) = buffer.read(index, position.fract());
            assert!((l - last).abs() < 0.02, "step at {position}");
            assert!((l + r).abs() < 1e-6);
            last = l;
        }
    }

    #[test]
    fn freezing_fades_the_writes_out_then_holds_the_head() {
        let mut buffer = Recording::new(48_000, 10_000);
        for _ in 0..10_000 {
            buffer.write(1.0, 1.0, true);
        }
        let fade = (48_000.0 * WRITE_FADE_SECONDS).round() as usize;
        for _ in 0..fade + 100 {
            buffer.write(0.0, 0.0, false);
        }
        assert_eq!(buffer.head(), fade);
        // Silence was written at a level falling steadily to nothing, so
        // the buffer crossfades from it back to what was there before.
        let written: Vec<f32> = (0..fade).map(|i| buffer.left[i]).collect();
        assert!(written.windows(2).all(|w| w[1] >= w[0]));
        assert!(written[0] < 0.01 && written[fade - 1] == 1.0);
    }

    #[test]
    fn a_whole_frame_reads_exactly() {
        let mut buffer = Recording::new(48_000, 8);
        for n in 0..8 {
            buffer.write(n as f32 * 0.1, 0.0, true);
        }
        assert_eq!(buffer.read(3, 0.0).0, 3.0 * 0.1);
    }
}
