//! Where the delay reads, and how a change of time moves it.
//!
//! A read head that jumps to a new time steps the waveform, which is heard
//! as a click. The delay offers the two cures that sound right:
//!
//! - **Crossfade**: a second head opens at the new time while the first
//!   fades out. Each head stays put while it plays, so the pitch never
//!   bends; a change that arrives mid-fade waits for the fade to end, then
//!   fades to the latest time.
//! - **Slew**: the one head glides to the new time, the way a tape delay's
//!   head moves, so the pitch bends while it travels. The glide runs through
//!   two smoothing stages, so its speed rises and falls gradually: the pitch
//!   has no step at the start or the end of a change.

/// How a change of time is heard, in the `glide` control's option order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Glide {
    Crossfade,
    Slew,
}

impl Glide {
    pub(super) fn from_position(position: u32) -> Option<Self> {
        match position {
            0 => Some(Self::Crossfade),
            1 => Some(Self::Slew),
            _ => None,
        }
    }
}

/// How long a crossfade between two read heads lasts.
const FADE_SECONDS: f32 = 0.02;
/// The time constant of each of the slew's two smoothing stages.
const SLEW_SECONDS: f64 = 0.1;
/// How close (in samples) a slewing head must come to its time to land
/// on it exactly: far below anything heard, and it ends the glide on the
/// whole sample a whole time asks for.
const LANDING: f64 = 1.0e-3;

/// One sample's reads: the offsets (in samples) of the head fading out and
/// the head fading in, and how far in the fade is (0 reads only `from`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Reads {
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) weight: f32,
}

/// The delay's read heads, owned by the audio thread.
pub(super) struct Heads {
    glide: Glide,
    /// A glide asked for while a crossfade runs, taken up when it ends.
    waiting: Option<Glide>,
    /// The time the controls ask for, in samples.
    target: f64,
    // Crossfade: the playing head, the head fading in, and the fade's
    // progress from 0 to 1 (`None` when no fade runs).
    from: f64,
    to: f64,
    fade: Option<f32>,
    fade_step: f32,
    // Slew: the two smoothing stages, the second being where the head is.
    first: f64,
    head: f64,
    slew_coefficient: f64,
}

impl Heads {
    /// Heads resting at `time` samples.
    pub(super) fn new(sample_rate: u32, time: f64, glide: Glide) -> Self {
        let rate = f64::from(sample_rate);
        let fade_frames = (FADE_SECONDS * sample_rate as f32).round().max(1.0);
        Self {
            glide,
            waiting: None,
            target: time,
            from: time,
            to: time,
            fade: None,
            fade_step: 1.0 / fade_frames,
            first: time,
            head: time,
            slew_coefficient: 1.0 - (-1.0 / (SLEW_SECONDS * rate)).exp(),
        }
    }

    /// Asks for `time` samples; the heads move there as the glide says.
    #[inline]
    pub(super) fn retarget(&mut self, time: f64) {
        self.target = time;
    }

    /// Moves to the asked-for time at once, with no glide: for a delay
    /// just built, which starts where its controls are.
    pub(super) fn settle(&mut self) {
        *self = Self {
            waiting: None,
            from: self.target,
            to: self.target,
            fade: None,
            first: self.target,
            head: self.target,
            ..*self
        };
    }

    /// Switches how changes are heard. A crossfade under way finishes
    /// first, so the switch never cuts a fade short; a slewing head hands
    /// over from wherever it has got to.
    pub(super) fn set_glide(&mut self, glide: Glide) {
        if glide == self.glide {
            self.waiting = None;
        } else if self.fade.is_some() {
            self.waiting = Some(glide);
        } else {
            self.switch(glide);
        }
    }

    fn switch(&mut self, glide: Glide) {
        match glide {
            Glide::Crossfade => {
                self.from = self.head;
                self.to = self.head;
            }
            Glide::Slew => {
                self.first = self.from;
                self.head = self.from;
            }
        }
        self.glide = glide;
    }

    /// This sample's reads, advancing the glide by one sample.
    #[inline]
    pub(super) fn next(&mut self) -> Reads {
        match self.glide {
            Glide::Crossfade => self.next_crossfade(),
            Glide::Slew => self.next_slew(),
        }
    }

    #[inline]
    fn next_crossfade(&mut self) -> Reads {
        let weight = match self.fade {
            Some(progress) => {
                let progress = progress + self.fade_step;
                if progress >= 1.0 {
                    self.from = self.to;
                    self.fade = None;
                    if let Some(glide) = self.waiting.take() {
                        self.switch(glide);
                    }
                    0.0
                } else {
                    self.fade = Some(progress);
                    progress
                }
            }
            None => {
                if self.target != self.from {
                    self.to = self.target;
                    self.fade = Some(0.0);
                }
                0.0
            }
        };
        Reads {
            from: self.from as f32,
            to: self.to as f32,
            weight,
        }
    }

    #[inline]
    fn next_slew(&mut self) -> Reads {
        if self.head != self.target {
            let k = self.slew_coefficient;
            self.first += k * (self.target - self.first);
            self.head += k * (self.first - self.head);
            if (self.head - self.target).abs() < LANDING
                && (self.first - self.target).abs() < LANDING
            {
                self.first = self.target;
                self.head = self.target;
            }
        }
        let at = self.head as f32;
        Reads {
            from: at,
            to: at,
            weight: 0.0,
        }
    }
}
