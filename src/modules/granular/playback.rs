//! The playback modes, and the crossfade from one to the next.
//!
//! Every mode reads the same recording; only the modes being heard run.
//! A mode change crossfades the old mode out and the new one in over
//! [`MODE_FADE_SECONDS`], equal in power since the two read different
//! audio. A change asked for mid-fade waits for the fade to finish.
//!
//! The looping delay is transposed after it is read, by a pitch shifter.
//!
//! The modes other than `granular` pass through a tone filter, as in
//! Mutable Instruments' `clouds/dsp/granular_processor.cc` (MIT, Copyright
//! 2014 Emilie Gillet): `texture` below 0.5 lowers a low-pass from the top
//! of the range nine octaves down, above 0.5 raises a high-pass from 16 Hz
//! up to 8 kHz. See THIRD_PARTY_NOTICES.md.

use crate::dsp::{
    cosine, sine, GrainCloud, GrainShape, LinearRamp, LoopShape, Looper, Onsets, PitchShifter,
    Recording, StretchShape, Stretcher, Svf,
};

/// How long a mode change takes.
const MODE_FADE_SECONDS: f32 = 0.02;
/// Frames between tone filter updates, as `texture` glides.
const TONE_EVERY: u32 = 16;
const TONE_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// The high-pass at `texture` 1, in hertz; the low-pass at 0.5 is the top
/// of the range. Each moves nine octaves.
const HIGHEST_HIGHPASS: f32 = 8_000.0;
const TONE_OCTAVES: f32 = 18.0;

/// A playback mode, in the order `mode` lists them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Mode {
    Granular,
    Stretch,
    LoopingDelay,
}

impl Mode {
    pub(super) fn from_index(index: u32) -> Option<Self> {
        match index {
            0 => Some(Self::Granular),
            1 => Some(Self::Stretch),
            2 => Some(Self::LoopingDelay),
            _ => None,
        }
    }
}

/// What one frame of playback is made from.
pub(super) struct Frame<'a> {
    pub(super) recording: &'a Recording,
    pub(super) shape: &'a GrainShape,
    pub(super) overlap: f32,
    pub(super) regular: bool,
    pub(super) onsets: Onsets,
    pub(super) trigger: bool,
    /// A synced clock's period in frames.
    pub(super) period: Option<u32>,
    /// Whether the write head is moving.
    pub(super) recording_on: bool,
}

/// Every mode's player, the mode heard, and the fade between modes.
pub(super) struct Playback {
    pub(super) grains: GrainCloud,
    stretcher: Stretcher,
    looper: Looper,
    shifter: PitchShifter,
    mode: Mode,
    /// The mode fading out, if a change is under way.
    from: Option<Mode>,
    fade: f32,
    fade_step: f32,
    pending: Option<Mode>,
    sample_rate: f32,
    tone: LinearRamp,
    countdown: u32,
    lowpass: [Svf; 2],
    highpass: [Svf; 2],
}

impl Playback {
    pub(super) fn new(sample_rate: u32, seed: u64, glide: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        let filter = |hz| Svf::new(sample_rate, hz, TONE_Q);
        let mut playback = Self {
            grains: GrainCloud::new(sample_rate, seed),
            stretcher: Stretcher::new(sample_rate),
            looper: Looper::new(sample_rate),
            shifter: PitchShifter::new(sample_rate),
            mode: Mode::Granular,
            from: None,
            fade: 0.0,
            fade_step: 1.0 / (MODE_FADE_SECONDS * rate).max(1.0),
            pending: None,
            sample_rate: rate,
            tone: LinearRamp::new(0.5, glide),
            countdown: 0,
            lowpass: [filter(rate), filter(rate)],
            highpass: [filter(1.0), filter(1.0)],
        };
        playback.update_tone(0.5);
        playback
    }

    /// The mode heard once any fade is done.
    pub(super) fn mode(&self) -> Mode {
        self.pending.unwrap_or(self.mode)
    }

    /// Asks for `mode`: a crossfade starts with the next block.
    pub(super) fn request(&mut self, mode: Mode) {
        self.pending = Some(mode);
    }

    /// Lets `position` set the looping delay again, after taps.
    pub(super) fn forget_taps(&mut self) {
        self.looper.forget_taps();
    }

    pub(super) fn set_texture(&mut self, texture: f32) {
        self.tone.set_target(texture);
    }

    /// Takes the asked-for mode and tone at once, as at build.
    pub(super) fn settle(&mut self) {
        if let Some(mode) = self.pending.take() {
            self.mode = mode;
        }
        self.tone.settle();
        self.update_tone(self.tone.value());
    }

    /// Starts a crossfade, at a block's start, if a mode was asked for.
    pub(super) fn begin_block(&mut self) {
        if self.from.is_some() {
            return;
        }
        if let Some(mode) = self.pending.take() {
            if mode != self.mode {
                self.from = Some(self.mode);
                self.mode = mode;
                self.fade = 0.0;
            }
        }
    }

    /// Recording restarts after a freeze: let go of what it would overrun.
    pub(super) fn thaw(&mut self, recording: &Recording) {
        self.grains.thaw(recording);
        self.stretcher.thaw(recording);
    }

    pub(super) fn tick(&mut self, frame: &Frame) -> (f32, f32) {
        if self.countdown == 0 {
            self.countdown = TONE_EVERY;
            let texture = self.tone.value();
            self.update_tone(texture);
        }
        self.countdown -= 1;
        self.tone.next_value();

        let (to, from) = match self.from {
            Some(old) => {
                self.fade += self.fade_step;
                if self.fade >= 1.0 {
                    self.from = None;
                    self.stop(old);
                    (1.0, 0.0)
                } else {
                    (sine(0.25 * self.fade), cosine(0.25 * self.fade))
                }
            }
            None => (1.0, 0.0),
        };
        let mut granular = (0.0, 0.0);
        let mut toned = (0.0, 0.0);
        let playing = [(Some(self.mode), to), (self.from, from)];
        for (mode, gain) in playing {
            let (left, right, filtered) = match mode {
                Some(Mode::Granular) => {
                    let (l, r) = self.grains.tick(
                        frame.recording,
                        frame.shape,
                        frame.overlap,
                        frame.regular,
                        frame.onsets,
                        frame.trigger,
                    );
                    (l, r, false)
                }
                Some(Mode::Stretch) => {
                    if frame.trigger {
                        self.stretcher.restart();
                    }
                    let shape = StretchShape {
                        position: frame.shape.position,
                        size: frame.shape.size,
                        ratio: frame.shape.ratio,
                    };
                    let (l, r) = self
                        .stretcher
                        .tick(frame.recording, &shape, frame.recording_on);
                    (l, r, true)
                }
                Some(Mode::LoopingDelay) => {
                    if frame.trigger {
                        self.looper.tap();
                    }
                    let shape = LoopShape {
                        position: frame.shape.position,
                        size: frame.shape.size,
                        period: frame.period,
                    };
                    let (l, r) = self
                        .looper
                        .tick(frame.recording, &shape, frame.recording_on);
                    let (l, r) = self.shifter.tick(l, r, frame.shape.ratio);
                    (l, r, true)
                }
                None => continue,
            };
            let sum = if filtered { &mut toned } else { &mut granular };
            sum.0 += gain * left;
            sum.1 += gain * right;
        }
        let left = self.highpass[0]
            .process(self.lowpass[0].process(toned.0).lowpass)
            .highpass;
        let right = self.highpass[1]
            .process(self.lowpass[1].process(toned.1).lowpass)
            .highpass;
        (granular.0 + left, granular.1 + right)
    }

    fn stop(&mut self, mode: Mode) {
        match mode {
            Mode::Granular => self.grains.clear(),
            Mode::Stretch => self.stretcher.clear(),
            Mode::LoopingDelay => {
                self.looper.clear();
                self.shifter.clear();
            }
        }
    }

    /// Sets the tone filters for `texture`.
    fn update_tone(&mut self, texture: f32) {
        let low = 0.5 * self.sample_rate * ((texture - 0.5).min(0.0) * TONE_OCTAVES).exp2();
        let high = HIGHEST_HIGHPASS * ((texture.max(0.5) - 1.0) * TONE_OCTAVES).exp2();
        for filter in &mut self.lowpass {
            filter.set(low, TONE_Q);
        }
        for filter in &mut self.highpass {
            filter.set(high, TONE_Q);
        }
    }
}
