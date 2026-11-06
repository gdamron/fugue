//! How a six-operator patch is written down, in our own units.
//!
//! A patch names its sound, picks one of the 32 [algorithms], and gives
//! each operator a frequency, a level and an envelope:
//!
//! - frequency: a ratio to the played note (1 is the note, 2 an octave up),
//!   or a fixed pitch in Hz, plus a detune in cents;
//! - level, in dB: for an operator that is heard, its loudness (0 dB is
//!   full scale); for a modulator, its modulation index relative to one
//!   radian (0 dB is an index of 1, +6 dB about 2, -6 dB about 0.5);
//! - envelope: seconds and decibels (see [`Shape`]).
//!
//! Optional touches: keyboard scaling (dB per octave either side of middle
//! C), rate scaling (envelopes running faster up the keyboard), velocity
//! sensitivity, tremolo depth, the patch's LFO, a pitch sweep at the start
//! of each note, and an output trim that levels the set.
//!
//! [algorithms]: super::algorithms

use super::algorithms::OPERATORS;
use super::envelope::{Shape, FLOOR_DB};
use super::lfo::LfoShape;

/// How an operator's frequency is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::modules::macro_oscillator) enum Tune {
    /// A multiple of the played note's frequency.
    Ratio(f32),
    /// A fixed frequency in Hz, whatever the note.
    Fixed(f32),
}

/// One operator, as authored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::modules::macro_oscillator) struct Op {
    pub tune: Tune,
    /// Cents either way.
    pub detune: f32,
    /// Loudness for a heard operator, modulation index re 1 radian for a
    /// modulator, in dB.
    pub level: f32,
    pub envelope: Shape,
    /// dB per octave below and above middle C (negative is quieter as the
    /// note moves away).
    pub keys_down: f32,
    pub keys_up: f32,
    /// How much faster the envelope runs per octave above middle C: 0 not
    /// at all, 1 twice as fast (and half as fast an octave below).
    pub rate_scale: f32,
    /// How many dB quieter the operator is at no accent than at the usual
    /// accent.
    pub velocity: f32,
    /// How much of the LFO's tremolo it takes, 0 to 1.
    pub tremolo: f32,
}

impl Op {
    /// An operator at `ratio` to the note and `level` dB, shaped by
    /// `envelope`, with no scaling.
    pub const fn new(ratio: f32, level: f32, envelope: Shape) -> Self {
        Self {
            tune: Tune::Ratio(ratio),
            detune: 0.0,
            level,
            envelope,
            keys_down: 0.0,
            keys_up: 0.0,
            rate_scale: 0.0,
            velocity: 6.0,
            tremolo: 0.0,
        }
    }

    /// An operator that is never heard and modulates nothing.
    pub const OFF: Self = Self::new(1.0, FLOOR_DB, decaying(0.001, 0.005));

    pub const fn fixed(self, hz: f32) -> Self {
        Self {
            tune: Tune::Fixed(hz),
            ..self
        }
    }

    pub const fn detune(self, cents: f32) -> Self {
        Self {
            detune: cents,
            ..self
        }
    }

    pub const fn keys(self, down: f32, up: f32) -> Self {
        Self {
            keys_down: down,
            keys_up: up,
            ..self
        }
    }

    pub const fn rate_scale(self, amount: f32) -> Self {
        Self {
            rate_scale: amount,
            ..self
        }
    }

    pub const fn velocity(self, db: f32) -> Self {
        Self {
            velocity: db,
            ..self
        }
    }

    pub const fn tremolo(self, amount: f32) -> Self {
        Self {
            tremolo: amount,
            ..self
        }
    }
}

/// A pitch offset that settles at the start of every note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::modules::macro_oscillator) struct PitchSweep {
    /// Where the pitch starts, in semitones from the note.
    pub semitones: f32,
    /// How long it takes to settle (to within a thousandth), in seconds.
    pub time: f32,
}

impl PitchSweep {
    pub const NONE: Self = Self {
        semitones: 0.0,
        time: 0.01,
    };
}

/// One patch, as authored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::modules::macro_oscillator) struct Patch {
    /// What it sounds like, in plain words.
    pub name: &'static str,
    /// 1 to 32.
    pub algorithm: u8,
    /// The feedback loop's depth, 0 to 1.
    pub feedback: f32,
    /// Operators 1 to 6.
    pub ops: [Op; OPERATORS],
    pub lfo: LfoShape,
    pub pitch: PitchSweep,
    /// An output trim in dB, so the patches sit at about the same loudness.
    pub gain: f32,
}

impl Patch {
    /// A patch called `name` on `algorithm` (1 to 32) with `feedback`, no
    /// LFO, no pitch sweep and no trim.
    pub const fn new(
        name: &'static str,
        algorithm: u8,
        feedback: f32,
        ops: [Op; OPERATORS],
    ) -> Self {
        Self {
            name,
            algorithm,
            feedback,
            ops,
            lfo: LfoShape::NONE,
            pitch: PitchSweep::NONE,
            gain: 0.0,
        }
    }

    pub const fn lfo(self, lfo: LfoShape) -> Self {
        Self { lfo, ..self }
    }

    /// Starts each note `semitones` off pitch, settling over `time` seconds.
    pub const fn sweep(self, semitones: f32, time: f32) -> Self {
        Self {
            pitch: PitchSweep { semitones, time },
            ..self
        }
    }

    pub const fn gain(self, db: f32) -> Self {
        Self { gain: db, ..self }
    }
}

/// An LFO: a sine at `rate` Hz with `vibrato` cents and `tremolo` dB,
/// fading in over `delay` seconds.
pub(in crate::modules::macro_oscillator) const fn lfo(
    rate: f32,
    delay: f32,
    vibrato: f32,
    tremolo: f32,
) -> LfoShape {
    LfoShape {
        wave: super::lfo::Wave::Sine,
        rate,
        delay,
        vibrato,
        tremolo,
    }
}

/// An envelope that rises over `attack` seconds and dies away, falling
/// 60 dB every `t60` seconds; let go, it falls at the same rate.
pub(in crate::modules::macro_oscillator) const fn decaying(attack: f32, t60: f32) -> Shape {
    Shape {
        attack,
        decay: t60,
        break_db: FLOOR_DB,
        second_decay: t60,
        sustain_db: FLOOR_DB,
        release: t60,
    }
}

/// A struck sound: a quick first fall (`fast` seconds per 60 dB) to
/// `break_db`, then a long tail (`slow`) to silence.
pub(in crate::modules::macro_oscillator) const fn struck(
    attack: f32,
    fast: f32,
    break_db: f32,
    slow: f32,
) -> Shape {
    Shape {
        attack,
        decay: fast,
        break_db,
        second_decay: slow,
        sustain_db: FLOOR_DB,
        release: slow,
    }
}

/// A held sound: it rises, settles at `sustain_db` at 60 dB per `t60`
/// seconds, and stays there while the key is down.
pub(in crate::modules::macro_oscillator) const fn sustained(
    attack: f32,
    t60: f32,
    sustain_db: f32,
) -> Shape {
    Shape {
        attack,
        decay: t60,
        break_db: sustain_db,
        second_decay: t60,
        sustain_db,
        release: 0.3,
    }
}

/// `shape` with its release (seconds per 60 dB) set.
pub(in crate::modules::macro_oscillator) const fn release(shape: Shape, t60: f32) -> Shape {
    Shape {
        release: t60,
        ..shape
    }
}

/// Why a patch can't be played, if it can't.
#[cfg(test)]
pub(in crate::modules::macro_oscillator) fn problem(patch: &Patch) -> Option<String> {
    let name = patch.name;
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c == ' ' || c == '-');
    if !plain {
        return Some(format!("`{name}`: a name is plain lowercase words"));
    }
    if !(1..=super::algorithms::ALGORITHM_COUNT as u8).contains(&patch.algorithm) {
        return Some(format!("`{name}`: algorithm {}", patch.algorithm));
    }
    if !(0.0..=1.0).contains(&patch.feedback) || !(-24.0..=12.0).contains(&patch.gain) {
        return Some(format!("`{name}`: feedback or gain out of range"));
    }
    for (i, op) in patch.ops.iter().enumerate() {
        let frequency_ok = match op.tune {
            Tune::Ratio(r) => (0.125..=32.0).contains(&r),
            Tune::Fixed(hz) => (0.1..=12_000.0).contains(&hz),
        };
        let e = op.envelope;
        let times = [e.attack, e.decay, e.second_decay, e.release];
        let ok = frequency_ok
            && op.detune.abs() <= 100.0
            && (FLOOR_DB..=24.0).contains(&op.level)
            && times.iter().all(|t| (0.0..=60.0).contains(t))
            && e.break_db <= 0.0
            && e.sustain_db <= 0.0
            && (-24.0..=24.0).contains(&op.keys_down)
            && (-24.0..=24.0).contains(&op.keys_up)
            && (-1.0..=2.0).contains(&op.rate_scale)
            && (0.0..=40.0).contains(&op.velocity)
            && (0.0..=1.0).contains(&op.tremolo);
        if !ok {
            return Some(format!("`{name}`: operator {} out of range", i + 1));
        }
    }
    let lfo = patch.lfo;
    let lfo_ok = (0.0..=40.0).contains(&lfo.rate)
        && (0.0..=10.0).contains(&lfo.delay)
        && (0.0..=200.0).contains(&lfo.vibrato)
        && (0.0..=24.0).contains(&lfo.tremolo)
        && patch.pitch.semitones.abs() <= 24.0
        && (0.001..=10.0).contains(&patch.pitch.time);
    if !lfo_ok {
        return Some(format!("`{name}`: LFO or pitch sweep out of range"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Patch {
        Patch::new(
            "plain sine",
            32,
            0.0,
            [
                Op::new(1.0, 0.0, decaying(0.001, 1.0)),
                Op::OFF,
                Op::OFF,
                Op::OFF,
                Op::OFF,
                Op::OFF,
            ],
        )
    }

    #[test]
    fn a_plain_patch_is_playable() {
        assert_eq!(problem(&plain()), None);
    }

    #[test]
    fn out_of_range_values_are_named() {
        let mut named = plain();
        named.name = "Plain Sine";
        assert!(problem(&named).unwrap().contains("lowercase"));
        let mut wired = plain();
        wired.algorithm = 33;
        assert!(problem(&wired).unwrap().contains("algorithm"));
        let mut loud = plain();
        loud.ops[0].level = 30.0;
        assert!(problem(&loud).unwrap().contains("operator 1"));
        let swept = plain().sweep(48.0, 0.1);
        assert!(problem(&swept).unwrap().contains("pitch sweep"));
    }

    #[test]
    fn the_shape_builders_release_as_described() {
        let struck = struck(0.002, 0.3, -12.0, 4.0);
        assert_eq!(
            (struck.attack, struck.decay, struck.break_db),
            (0.002, 0.3, -12.0)
        );
        assert_eq!((struck.second_decay, struck.release), (4.0, 4.0));
        let held = sustained(0.05, 2.0, -6.0);
        assert_eq!((held.sustain_db, held.release), (-6.0, 0.3));
        assert_eq!(release(held, 1.5).release, 1.5);
    }
}
