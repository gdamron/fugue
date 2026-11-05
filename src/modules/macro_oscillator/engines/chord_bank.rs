//! The chords the `chords` model plays, built in a key and scale.
//!
//! The inversion voicing and the chord choice's hysteresis are ported
//! from Mutable Instruments' `plaits/dsp/chords/chord_bank.cc` (MIT,
//! Copyright 2021 Emilie Gillet) and `stmlib/dsp/hysteresis_quantizer.h`
//! (MIT, Copyright 2015 Emilie Gillet). See THIRD_PARTY_NOTICES.md. The
//! chords themselves are Fugue's: shapes counted in scale steps, so their
//! quality comes from the scale.

use crate::modules::macro_oscillator::engine::semitones_to_ratio;
use crate::music::scales::{in_key, nearest_in, scale_degrees};

/// Notes in a chord, and the voices that play them: one more, so a note
/// can crossfade between octaves as the inversion moves.
pub(super) const NOTES: usize = 4;
pub(super) const VOICES: usize = NOTES + 1;

/// One chord note: scale steps above the root (counted in a seven-note
/// scale and scaled to the scale's size), whole octaves added, and a
/// detune in cents that thickens a doubled note.
#[derive(Clone, Copy)]
struct Tone(u8, u8, f32);

/// The chord shapes, in `harmonics` order: plain to rich.
#[rustfmt::skip]
const SHAPES: [[Tone; NOTES]; 10] = [
    // Octave, doubled and detuned.
    [Tone(0, 0, 0.0), Tone(0, 0, 1.0), Tone(0, 1, -1.0), Tone(0, 1, 0.0)],
    // Fifth.
    [Tone(0, 0, 0.0), Tone(4, 0, 0.0), Tone(4, 0, 1.0), Tone(0, 1, 0.0)],
    // Suspended second and fourth.
    [Tone(0, 0, 0.0), Tone(1, 0, 0.0), Tone(4, 0, 0.0), Tone(0, 1, 0.0)],
    [Tone(0, 0, 0.0), Tone(3, 0, 0.0), Tone(4, 0, 0.0), Tone(0, 1, 0.0)],
    // Triad.
    [Tone(0, 0, 0.0), Tone(2, 0, 0.0), Tone(4, 0, 0.0), Tone(0, 1, 0.0)],
    // Sixth and seventh.
    [Tone(0, 0, 0.0), Tone(2, 0, 0.0), Tone(4, 0, 0.0), Tone(5, 0, 0.0)],
    [Tone(0, 0, 0.0), Tone(2, 0, 0.0), Tone(4, 0, 0.0), Tone(6, 0, 0.0)],
    // Ninth and eleventh, without the fifth.
    [Tone(0, 0, 0.0), Tone(2, 0, 0.0), Tone(6, 0, 0.0), Tone(8, 0, 0.0)],
    [Tone(0, 0, 0.0), Tone(2, 0, 0.0), Tone(6, 0, 0.0), Tone(10, 0, 0.0)],
    // Six-nine.
    [Tone(0, 0, 0.0), Tone(1, 0, 0.0), Tone(5, 0, 0.0), Tone(9, 0, 0.0)],
];
const SHAPE_COUNT: usize = SHAPES.len();
/// How far past a boundary `harmonics` must go to change chord.
const HYSTERESIS: f32 = 0.075;
/// Inversion steps across `timbre`: four octaves, so the middle is root
/// position at the played pitch.
const INVERSIONS: f32 = 16.0;
/// Each voice's level.
const VOICE_GAIN: f32 = 0.25;

/// What the cached ratios were built for.
#[derive(Clone, Copy, PartialEq)]
struct Built {
    shape: usize,
    key: u8,
    scale: u8,
    note: i32,
}

pub(super) struct ChordBank {
    shape: usize,
    built: Option<Built>,
    /// Each chord note's frequency ratio to the root.
    ratios: [f32; NOTES],
}

/// One block's voicing: each voice's ratio to the played pitch and
/// level, and which voices carry the root (bit per voice).
pub(super) struct Voicing {
    pub ratios: [f32; VOICES],
    pub levels: [f32; VOICES],
    pub root_mask: u8,
}

impl ChordBank {
    pub fn new() -> Self {
        Self {
            shape: 0,
            built: None,
            ratios: [1.0; NOTES],
        }
    }

    /// The chord shape last picked.
    #[cfg(test)]
    pub fn shape(&self) -> usize {
        self.shape
    }

    /// Picks the chord for `harmonics`, with hysteresis so a knob resting
    /// on a boundary doesn't flicker.
    pub fn select(&mut self, harmonics: f32) {
        let value = harmonics * 1.02 * SHAPE_COUNT as f32 - 0.5;
        let sign = if value > self.shape as f32 { -1.0 } else { 1.0 };
        let q = (value + sign * HYSTERESIS + 0.5).max(0.0) as usize;
        self.shape = q.min(SHAPE_COUNT - 1);
    }

    /// Builds the chord on `note` in `key` and `scale`: the intervals are
    /// those of the chord on the scale degree nearest `note`, played from
    /// `note` itself, so an off-scale note shifts that chord in parallel.
    /// Costs nothing unless something changed.
    pub fn build(&mut self, note: f32, key: u8, scale: u8) {
        let note = if note.is_finite() {
            note.round() as i32
        } else {
            60
        };
        let wanted = Built {
            shape: self.shape,
            key,
            scale,
            note,
        };
        if self.built == Some(wanted) {
            return;
        }
        self.built = Some(wanted);
        let degrees = scale_degrees(scale as usize);
        let size = degrees.len();
        let mask = degrees.iter().fold(0u16, |m, &d| m | 1 << d);
        let nearest = i32::from(nearest_in(in_key(mask, key), note as f32));
        let class = (nearest - i32::from(key)).rem_euclid(12) as u8;
        let root = degrees.iter().position(|&d| d == class).unwrap_or(0);
        for (ratio, &Tone(steps, octaves, cents)) in self.ratios.iter_mut().zip(&SHAPES[self.shape])
        {
            // Steps counted in a seven-note scale, scaled to this one.
            let index = root + (usize::from(steps) * size * 2 + 7) / 14;
            let semitones = i32::from(degrees[index % size]) - i32::from(degrees[root])
                + 12 * (index / size) as i32
                + 12 * i32::from(octaves);
            *ratio = semitones_to_ratio(semitones as f32 + cents * 0.01);
        }
    }

    /// Spreads the chord over the voices for an inversion of 0 to 1: from
    /// two octaves down, through root position at the middle, to two
    /// octaves up, crossfading each note into its new octave.
    pub fn voice(&self, inversion: f32) -> Voicing {
        let mut v = Voicing {
            ratios: [1.0; VOICES],
            levels: [0.0; VOICES],
            root_mask: 0,
        };
        let inversion = inversion.clamp(0.0, 1.0) * INVERSIONS;
        let integral = inversion as usize;
        let fractional = inversion - integral as f32;
        let rotations = integral / NOTES;
        let rotated = integral % NOTES;
        for i in 0..NOTES {
            let octave = (NOTES - 1 + integral - i) / NOTES;
            let transposition = 0.25 * (1u32 << octave) as f32;
            let target = (i + VOICES * 4 - rotations) % VOICES;
            let previous = (target + VOICES - 1) % VOICES;
            let ratio = self.ratios[i] * transposition;
            if i == rotated {
                v.ratios[target] = ratio;
                v.ratios[previous] = ratio * 2.0;
                v.levels[previous] = VOICE_GAIN * fractional;
                v.levels[target] = VOICE_GAIN * (1.0 - fractional);
            } else if i < rotated {
                v.ratios[previous] = ratio;
                v.levels[previous] = VOICE_GAIN;
            } else {
                v.ratios[target] = ratio;
                v.levels[target] = VOICE_GAIN;
            }
            if i == 0 {
                if i >= rotated {
                    v.root_mask |= 1 << target;
                }
                if i <= rotated {
                    v.root_mask |= 1 << previous;
                }
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn semitones(bank: &ChordBank) -> Vec<i32> {
        bank.ratios
            .iter()
            .map(|r| (12.0 * r.log2()).round() as i32)
            .collect()
    }

    #[test]
    fn a_triad_takes_its_quality_from_the_scale() {
        let mut bank = ChordBank::new();
        bank.select(4.5 / 10.0 / 1.02);
        // C major: on C a major triad, on D a minor one, on B diminished.
        bank.build(60.0, 0, 0);
        assert_eq!(semitones(&bank), [0, 4, 7, 12]);
        bank.build(62.0, 0, 0);
        assert_eq!(semitones(&bank), [0, 3, 7, 12]);
        bank.build(71.0, 0, 0);
        assert_eq!(semitones(&bank), [0, 3, 6, 12]);
        // A minor on A, and on E in harmonic minor, a major triad.
        bank.build(69.0, 9, 1);
        assert_eq!(semitones(&bank), [0, 3, 7, 12]);
        bank.build(64.0, 9, 2);
        assert_eq!(semitones(&bank), [0, 4, 7, 12]);
    }

    #[test]
    fn harmonics_steps_through_the_shapes_with_hysteresis() {
        let mut bank = ChordBank::new();
        let mut seen = Vec::new();
        for i in 0..=100 {
            bank.select(i as f32 / 100.0);
            if seen.last() != Some(&bank.shape) {
                seen.push(bank.shape);
            }
        }
        assert_eq!(seen, (0..SHAPE_COUNT).collect::<Vec<_>>());
        // Just across the boundary between shapes 4 and 5, either way,
        // holds the chord; further changes it.
        let at = |position: f32| (position + 0.5) / (1.02 * SHAPE_COUNT as f32);
        bank.select(at(4.0));
        bank.select(at(4.55));
        assert_eq!(bank.shape, 4);
        bank.select(at(4.7));
        assert_eq!(bank.shape, 5);
        bank.select(at(4.45));
        assert_eq!(bank.shape, 5);
    }

    #[test]
    fn the_middle_inversion_is_root_position_at_the_played_pitch() {
        let mut bank = ChordBank::new();
        bank.select(0.5);
        bank.build(57.0, 0, 0);
        let v = bank.voice(0.5);
        let sounding: Vec<f32> = (0..VOICES)
            .filter(|&k| v.levels[k] > 0.0)
            .map(|k| v.ratios[k])
            .collect();
        assert_eq!(sounding.len(), NOTES);
        let lowest = sounding.iter().copied().fold(f32::MAX, f32::min);
        assert_eq!(lowest, 1.0);
        // Either end moves the chord two octaves.
        let low = bank.voice(0.0);
        assert!(low
            .ratios
            .iter()
            .zip(&low.levels)
            .any(|(&r, &l)| l > 0.0 && r == 0.25));
    }

    /// `aux` carries the root alone: every voice in the root mask plays
    /// the root in some octave, at every inversion.
    #[test]
    fn the_root_mask_holds_only_the_root() {
        let mut bank = ChordBank::new();
        bank.select(0.5);
        bank.build(60.0, 0, 0);
        for i in 0..=64 {
            let v = bank.voice(i as f32 / 64.0);
            for k in (0..VOICES).filter(|&k| v.root_mask & 1 << k != 0) {
                let octaves = v.ratios[k].log2();
                assert!(
                    (octaves - octaves.round()).abs() < 1e-4,
                    "{i} {k} {octaves}"
                );
            }
            let others = (0..VOICES).filter(|&k| v.root_mask & 1 << k == 0);
            assert!(others
                .filter(|&k| v.levels[k] > 0.0)
                .all(|k| (v.ratios[k].log2() - v.ratios[k].log2().round()).abs() > 1e-3));
        }
    }

    #[test]
    fn levels_always_sum_to_four_voices() {
        let mut bank = ChordBank::new();
        bank.build(60.0, 0, 0);
        for i in 0..=64 {
            let v = bank.voice(i as f32 / 64.0);
            let total: f32 = v.levels.iter().sum();
            assert!((total - NOTES as f32 * VOICE_GAIN).abs() < 1e-5, "{total}");
            assert!(v.root_mask != 0);
        }
    }
}
