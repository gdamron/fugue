//! The contour's voice: the generator that is playing, and, for a moment
//! after a mode change, the one it is fading from.

use super::generator::{Generator, Modes, Params};
use super::ramp::{Drive, CHANNELS};
use super::tables::Tables;
use super::RampMode;

/// What the trigger and clock inputs did this sample.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Sense {
    pub trigger_patched: bool,
    pub trigger_rose: bool,
    pub trigger_high: bool,
    pub clock_rose: bool,
    pub clock_high: bool,
    /// The phase through the clock's period, once it has one.
    pub clock_phase: Option<f32>,
}

impl Sense {
    /// How the inputs drive a ramp in `mode`. A clock sets a loop's cycle
    /// even with a trigger patched (which then restarts it); an envelope
    /// follows the clock only when no trigger is patched.
    fn drive(&self, mode: RampMode) -> Drive {
        if self.trigger_patched {
            Drive {
                rising: self.trigger_rose,
                high: self.trigger_high,
                clock: if mode == RampMode::Looping {
                    self.clock_phase
                } else {
                    None
                },
            }
        } else {
            // A loop follows the clock's phase once it has a period; until
            // then each edge restarts it.
            Drive {
                rising: self.clock_rose
                    && (mode != RampMode::Looping || self.clock_phase.is_none()),
                high: self.clock_high,
                clock: self.clock_phase,
            }
        }
    }
}

/// How many cycles a mode change crossfades over, and the shortest and
/// longest crossfade in seconds.
const FADE_CYCLES: f32 = 2.0;
const MIN_FADE_SECONDS: f32 = 0.02;
const MAX_FADE_SECONDS: f32 = 4.0;

pub(super) struct Voice {
    tables: &'static Tables,
    current: Generator,
    previous: Generator,
    /// Frames left in the crossfade from `previous`, and its length.
    fade_left: u32,
    fade_frames: u32,
    min_fade: f32,
    max_fade: f32,
    /// The modes asked for. A change waits for any crossfade in progress
    /// to finish, so the voice never jumps from one fade to the next.
    pub modes: Modes,
}

impl Voice {
    pub(super) fn new(tables: &'static Tables, sample_rate: f32, modes: Modes) -> Self {
        Self {
            tables,
            current: Generator::new(modes),
            previous: Generator::new(modes),
            fade_left: 0,
            fade_frames: 1,
            min_fade: MIN_FADE_SECONDS * sample_rate,
            max_fade: MAX_FADE_SECONDS * sample_rate,
            modes,
        }
    }

    /// Starts over in `modes`, with no crossfade (before anything is heard).
    pub(super) fn restart(&mut self, modes: Modes) {
        self.modes = modes;
        self.current = Generator::new(modes);
        self.fade_left = 0;
    }

    /// The four outputs for the next sample.
    #[inline]
    pub(super) fn render(&mut self, params: &Params, sense: &Sense) -> [f32; CHANNELS] {
        if self.fade_left == 0 && self.modes != self.current.modes {
            self.previous = self.current;
            self.current = self.current.retuned(self.modes);
            let cycles = FADE_CYCLES / params.frequency.max(1.0e-9);
            self.fade_frames = cycles.clamp(self.min_fade, self.max_fade) as u32;
            self.fade_left = self.fade_frames;
        }
        let drive = sense.drive(self.current.modes.ramp);
        let now = self.current.render(self.tables, params, drive);
        if self.fade_left == 0 {
            return now;
        }
        let drive = sense.drive(self.previous.modes.ramp);
        let before = self.previous.render(self.tables, params, drive);
        self.fade_left -= 1;
        let t = 1.0 - self.fade_left as f32 / self.fade_frames as f32;
        std::array::from_fn(|k| before[k] + (now[k] - before[k]) * t)
    }
}
