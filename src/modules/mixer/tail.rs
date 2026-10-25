//! What a mixer plays out for a channel that stops being heard: one an
//! edit removes, or whose input goes with the module that fed it.
//!
//! The channel's source is gone by the time the mixer hears of it, so there
//! is no more of its signal to fade. Stopping it at once steps the
//! waveform, which clicks. Instead the mixer carries the channel on from
//! its last two samples: the value it was at, coasting on the slope it was
//! moving at as that slope dies away, so neither the value nor the slope
//! jumps. That fades out over the same glide as a level change
//! ([`GAIN_RAMP_MS`](super::GAIN_RAMP_MS)). Every departing channel's tail
//! is summed into one stereo pair before the master.

use super::MAX_CHANNELS;
use crate::dsp::LinearRamp;

/// How much of its slope a coasting tail keeps from one frame to the next.
/// The slope dies away within a few dozen frames, so the tail settles near
/// the value it left at and fades from there.
const SLOPE_DECAY: f32 = 0.9;

/// The furthest a coasting tail may stray from the value it left at, as
/// its slope dies away: a slope that would carry it further (bright
/// content) is limited to what reaches this.
const MAX_COAST: f32 = 0.25;

/// How many of a block's last frames a channel's peak is taken over. A
/// coast never carries past the channel's peak over them, so a quiet
/// channel never leaves a loud tail: these frames are about as long as the
/// coast's own slope takes to die away.
const PEAK_FRAMES: usize = 32;

/// The slope that coasts one unit as it dies away.
const SLOPE_SCALE: f32 = (1.0 - SLOPE_DECAY) / SLOPE_DECAY;

/// The steepest slope a tail starts with: the one that coasts [`MAX_COAST`].
const MAX_SLOPE: f32 = MAX_COAST * SLOPE_SCALE;

/// One side of a coasting tail: where it is and how fast it is moving.
#[derive(Clone, Copy, Debug, Default)]
struct Coast {
    value: f32,
    slope: f32,
}

impl Coast {
    /// A tail leaving `value`, `slope` per frame, at `gain`. One that is not
    /// a number, or a gain that is not, leaves silent.
    fn leaving(value: f32, slope: f32, gain: f32) -> Self {
        let (value, slope) = (value * gain, slope * gain);
        if value.is_finite() && slope.is_finite() {
            Self { value, slope }
        } else {
            Self::default()
        }
    }

    fn add(&mut self, other: Self) {
        self.value += other.value;
        self.slope += other.slope;
    }

    /// Scales where it is and how fast it moves alike: a fade folded in.
    fn scaled(self, gain: f32) -> Self {
        Self {
            value: self.value * gain,
            slope: self.slope * gain,
        }
    }

    #[inline]
    fn next_value(&mut self) -> f32 {
        self.slope *= SLOPE_DECAY;
        self.value += self.slope;
        self.value
    }
}

/// A channel's tail between the edit that removes it and the next block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Waiting {
    #[default]
    None,
    /// Plays out from the next block, unless its input is connected first.
    Pending,
    /// Its input was connected again before a block played: it goes on
    /// being heard, so it has no tail. Kept so an edit installing in the
    /// same block on top of this one decides afresh.
    Covered,
}

/// A stereo tail waiting to play out.
#[derive(Clone, Copy, Debug, Default)]
struct Departure {
    waiting: Waiting,
    left: Coast,
    right: Coast,
}

/// What a mixer last heard of a channel's input (after its level CV).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Heard {
    last: f32,
    before_last: f32,
    /// The largest magnitude over the block's last [`PEAK_FRAMES`] frames.
    peak: f32,
}

/// A mixer's departing channels: those waiting for the next block, and the
/// stereo tail they play out as.
pub(super) struct Tails {
    /// What each channel was heard at as the last block ended.
    heard: [Heard; MAX_CHANNELS],
    /// Whether `heard` holds what this mixer last played, rather than
    /// nothing (it has not played yet).
    remembered: bool,
    departures: [Departure; MAX_CHANNELS],
    any_waiting: bool,
    left: Coast,
    right: Coast,
    /// The fade the tail plays out under, from 1 to 0.
    fade: LinearRamp,
}

impl Tails {
    /// No tail, fading over `frames` once there is one.
    pub(super) fn new(frames: u32) -> Self {
        Self {
            heard: [Heard::default(); MAX_CHANNELS],
            remembered: false,
            departures: [Departure::default(); MAX_CHANNELS],
            any_waiting: false,
            left: Coast::default(),
            right: Coast::default(),
            fade: LinearRamp::new(0.0, frames),
        }
    }

    /// Records what each channel's input was heard at, after a block.
    #[inline]
    pub(super) fn remember(
        &mut self,
        channels: usize,
        frames: usize,
        sample: impl Fn(usize, usize) -> f32,
    ) {
        if frames == 0 {
            return;
        }
        for (ch, heard) in self.heard[..channels].iter_mut().enumerate() {
            let last = sample(ch, frames - 1);
            let before_last = if frames > 1 {
                sample(ch, frames - 2)
            } else {
                heard.last
            };
            // A block shorter than two frames also keeps the one before.
            let mut peak = last.abs().max(before_last.abs());
            for i in frames.saturating_sub(PEAK_FRAMES)..frames.saturating_sub(2) {
                peak = peak.max(sample(ch, i).abs());
            }
            *heard = Heard {
                last,
                before_last,
                peak,
            };
        }
        self.remembered = true;
    }

    /// Channel `ch`'s input is connected: it goes on being heard, so a
    /// tail waiting for it is dropped.
    pub(super) fn cover(&mut self, ch: usize) {
        if let Some(departure) = self.departures.get_mut(ch) {
            if departure.waiting == Waiting::Pending {
                departure.waiting = Waiting::Covered;
            }
        }
    }

    /// Channel `ch`'s input loses its connection: a tail its connection
    /// covered waits to play out again. Returns whether there was one.
    pub(super) fn uncover(&mut self, ch: usize) -> bool {
        match self.departures.get_mut(ch) {
            Some(departure) if departure.waiting == Waiting::Covered => {
                departure.waiting = Waiting::Pending;
                self.any_waiting = true;
                true
            }
            _ => false,
        }
    }

    /// Whether this mixer has played a block and remembers its last one.
    pub(super) fn remembered(&self) -> bool {
        self.remembered
    }

    /// Takes over `previous`'s tail as it plays, and the tails waiting in
    /// it if it never played, to be decided afresh here. Its own channels
    /// are left for the caller to [`depart`](Self::depart) from what
    /// `previous` remembers, when it played.
    pub(super) fn carry_on_from(&mut self, previous: &Tails) {
        self.left = previous.left;
        self.right = previous.right;
        self.fade = previous.fade;
        if !previous.remembered {
            self.any_waiting = previous.any_waiting;
            for (mine, theirs) in self.departures.iter_mut().zip(&previous.departures) {
                *mine = Departure {
                    waiting: match theirs.waiting {
                        Waiting::None => Waiting::None,
                        _ => Waiting::Pending,
                    },
                    ..*theirs
                };
            }
        }
    }

    /// What this mixer last heard of channel `ch`.
    pub(super) fn heard(&self, ch: usize) -> Heard {
        self.heard[ch]
    }

    /// Starts channel `ch` leaving from what was last `heard` of it (see
    /// [`Self::heard`]), at left and right `gains` (its level and pan as
    /// last heard). A silent channel leaves nothing.
    pub(super) fn depart(&mut self, ch: usize, heard: Heard, gains: (f32, f32)) {
        let Heard {
            last,
            before_last,
            peak,
        } = heard;
        if (last == 0.0 && before_last == 0.0) || ch >= MAX_CHANNELS {
            return;
        }
        // The coast stays within the channel's recent peak. A slope that
        // is not a number leaves the value alone (see `Coast::leaving`).
        let room = (peak - last.abs()).max(0.0) * SLOPE_SCALE;
        let steepest = if room.is_finite() { room } else { 0.0 };
        let slope = (last - before_last).max(-steepest).min(steepest);
        self.departures[ch] = Departure {
            waiting: Waiting::Pending,
            left: Coast::leaving(last, slope, gains.0),
            right: Coast::leaving(last, slope, gains.1),
        };
        self.any_waiting = true;
    }

    /// Before a block: folds every tail still waiting into the one playing
    /// out, which fades from there over a whole glide again.
    #[inline]
    pub(super) fn start_block(&mut self) {
        if !self.any_waiting {
            return;
        }
        self.any_waiting = false;
        let fade = self.fade.value();
        let mut left = self.left.scaled(fade);
        let mut right = self.right.scaled(fade);
        let mut joined = false;
        for departure in &mut self.departures {
            if departure.waiting == Waiting::Pending {
                left.add(departure.left);
                right.add(departure.right);
                joined = true;
            }
            departure.waiting = Waiting::None;
        }
        if joined {
            self.left = limited(left);
            self.right = limited(right);
            self.fade.snap(1.0);
            self.fade.set_target(0.0);
        }
    }

    /// Whether a tail is playing out.
    #[inline]
    pub(super) fn playing(&self) -> bool {
        self.fade.is_gliding()
    }

    /// The tail's next stereo frame.
    #[inline]
    pub(super) fn next_frame(&mut self) -> (f32, f32) {
        let fade = self.fade.next_value();
        (
            self.left.next_value() * fade,
            self.right.next_value() * fade,
        )
    }
}

/// `coast` with its slope limited to [`MAX_SLOPE`].
fn limited(coast: Coast) -> Coast {
    Coast {
        slope: coast.slope.clamp(-MAX_SLOPE, MAX_SLOPE),
        ..coast
    }
}
