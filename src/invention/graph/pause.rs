//! Pausing the graph: the output fades out, the graph holds, and on resume
//! it plays on from where it stood as the output fades back in.
//!
//! A pause or resume is a request on the transport (see
//! [`Transport::request_pause`](crate::control_request::Transport::request_pause)),
//! taken at the start of each block. On a pause the graph keeps playing
//! for the length of the fade, [`PAUSE_FADE_SECONDS`], while the mixed
//! output's gain falls linearly to zero, and stops on the fade's last
//! frame: the rest of that block is silence. While it holds, every block
//! renders silence and runs as a zero-length block does (see
//! `SignalGraph::process_block`): requests, input writes and edits are
//! taken and applied at the held sample, but no module runs and the sample
//! count stands still, so clocks, envelopes, ramps and anything timed on
//! the count or on a clock's beats wait. On a resume the graph plays again
//! from the next block, its gain rising from zero over the same length. A
//! request that comes mid-fade turns the fade around from the gain it has
//! reached.
//!
//! The fade lasts as long as an unpatched input's release, so a pause is
//! heard as the same short, click-free release. It scales each sample as
//! the device would play it (clamped and finite, see [`device_sample`]),
//! so a mix driven past full scale still falls in a straight line rather
//! than holding at the clamp and then dropping fast. Outside a fade the
//! output is untouched.

use super::releases::RELEASE_SECONDS;
use crate::modules::dac::device_sample;

/// How long the output fades out on a pause and in on a resume.
pub(crate) const PAUSE_FADE_SECONDS: f32 = RELEASE_SECONDS;

/// The fade's length at 48 kHz, until the graph is given its rate.
const DEFAULT_FADE_FRAMES: u32 = 480;

/// Where the graph stands between playing and held.
#[derive(Debug)]
pub(crate) struct Pause {
    /// Frames in a whole fade, at least one.
    frames: u32,
    /// The output's gain in frames of the fade: `frames` is full gain and
    /// 0 silence. Counted in whole frames, so a fade lands exactly.
    level: u32,
    /// Whether the graph is pausing or held, rather than playing.
    pausing: bool,
}

impl Default for Pause {
    fn default() -> Self {
        Self {
            frames: DEFAULT_FADE_FRAMES,
            level: DEFAULT_FADE_FRAMES,
            pausing: false,
        }
    }
}

impl Pause {
    /// Times the fade for `sample_rate`, keeping where it stands. Not the
    /// audio hot path.
    pub(super) fn set_sample_rate(&mut self, sample_rate: u32) {
        let frames = ((sample_rate as f32 * PAUSE_FADE_SECONDS).round() as u32).max(1);
        self.level = if self.level == self.frames {
            frames
        } else {
            (u64::from(self.level) * u64::from(frames) / u64::from(self.frames)) as u32
        };
        self.frames = frames;
    }

    /// Takes the latest request (`paused`) at the start of a block of
    /// `frames` frames, and returns how many of them the graph plays: all
    /// of them while it plays or fades in, what is left of the fade out
    /// while it pauses, and none while it holds.
    #[inline]
    pub(super) fn frames_to_play(&mut self, paused: bool, frames: usize) -> usize {
        self.pausing = paused;
        if paused {
            frames.min(self.level as usize)
        } else {
            frames
        }
    }

    /// Whether the graph holds: paused, and faded all the way out.
    #[cfg(test)]
    pub(super) fn holds(&self) -> bool {
        self.pausing && self.level == 0
    }

    /// Fades the mixed output a segment holds, one frame of the fade per
    /// frame, while a fade runs; leaves it untouched otherwise.
    /// Allocation- and lock-free.
    #[inline]
    pub(super) fn fade(&mut self, left: &mut [f32], right: &mut [f32]) {
        if !self.pausing && self.level == self.frames {
            return;
        }
        let frames = self.frames as f32;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.level = if self.pausing {
                self.level.saturating_sub(1)
            } else {
                (self.level + 1).min(self.frames)
            };
            let gain = self.level as f32 / frames;
            *l = device_sample(*l) * gain;
            *r = device_sample(*r) * gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gains `frames` frames of the fade apply, read off a half-scale
    /// signal (full scale would read the device's clamp).
    fn faded(pause: &mut Pause, frames: usize) -> Vec<f32> {
        let mut left = vec![0.5; frames];
        let mut right = vec![0.5; frames];
        pause.fade(&mut left, &mut right);
        assert_eq!(left, right);
        left.iter().map(|sample| sample * 2.0).collect()
    }

    #[test]
    fn playing_leaves_the_output_untouched() {
        let mut pause = Pause::default();
        assert_eq!(pause.frames_to_play(false, 64), 64);
        let mut left = [2.0, f32::NAN, -0.5];
        let mut right = left;
        pause.fade(&mut left, &mut right);
        assert_eq!(left[0], 2.0);
        assert!(left[1].is_nan());
    }

    #[test]
    fn a_pause_falls_linearly_to_silence_over_the_fade_then_holds() {
        let mut pause = Pause::default();
        pause.set_sample_rate(48_000);
        let mut out = Vec::new();
        for _ in 0..10 {
            let frames = pause.frames_to_play(true, 64);
            out.extend(faded(&mut pause, frames));
        }
        assert_eq!(out.len(), 480, "it plays exactly the fade");
        assert_eq!(out[0], 479.0 / 480.0);
        assert_eq!(out[479], 0.0);
        assert!(out
            .windows(2)
            .all(|w| (w[0] - w[1] - 1.0 / 480.0).abs() < 1e-6));
        assert!(pause.holds());
        assert_eq!(pause.frames_to_play(true, 64), 0);
    }

    #[test]
    fn a_resume_rises_from_silence_and_a_pause_mid_fade_turns_it_around() {
        let mut pause = Pause::default();
        loop {
            let frames = pause.frames_to_play(true, 64);
            if frames == 0 {
                break;
            }
            faded(&mut pause, frames);
        }
        assert_eq!(pause.frames_to_play(false, 64), 64);
        let rising = faded(&mut pause, 100);
        assert_eq!(rising[0], 1.0 / 480.0);
        assert_eq!(rising[99], 100.0 / 480.0);
        // Turned around at 100 frames up: 100 frames down to silence.
        assert_eq!(pause.frames_to_play(true, 400), 100);
        let falling = faded(&mut pause, 100);
        assert_eq!(falling[0], 99.0 / 480.0);
        assert_eq!(falling[99], 0.0);
    }

    #[test]
    fn a_fade_scales_the_device_sample() {
        let mut pause = Pause::default();
        pause.frames_to_play(true, 1);
        let mut left = [4.0];
        let mut right = [f32::INFINITY];
        pause.fade(&mut left, &mut right);
        assert!(left[0] < 1.0 && left[0] > 0.99);
        assert_eq!(right[0], 0.0);
    }

    #[test]
    fn a_new_rate_keeps_where_the_fade_stands() {
        let mut pause = Pause::default();
        pause.set_sample_rate(96_000);
        assert_eq!((pause.frames, pause.level), (960, 960));
        pause.frames_to_play(true, 480);
        faded(&mut pause, 480);
        pause.set_sample_rate(48_000);
        assert_eq!((pause.frames, pause.level), (480, 240));
    }
}
