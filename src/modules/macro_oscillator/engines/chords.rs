//! Ported from Mutable Instruments' `plaits/dsp/engine/chord_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet), adapted to Fugue's block
//! processing. See THIRD_PARTY_NOTICES.md.
//!
//! Chords in key: a divide-down organ and string machine plays a
//! four-note chord on the played pitch, built from the `key` and `scale`
//! controls. `harmonics` picks the chord (octave, fifth, suspended,
//! triad, sixth, seventh, ninth, eleventh, six-nine), `timbre` its
//! inversion, and `morph` the organ registration. `aux` is the root alone.

use super::chord_bank::{ChordBank, VOICES};
use crate::modules::macro_oscillator::engine::EngineParameters;
use crate::modules::macro_oscillator::oscillators::{StringSynthOscillator, STOPS};

/// The organ registrations `morph` sweeps (saw and square at 8', 4' and
/// 2'), from bright to hollow.
const REGISTRATIONS: [[f32; 6]; 8] = [
    [0.0, 0.1, 0.1, 0.0, 0.2, 0.6],    // Saw and square with high square
    [0.0, 0.5, 0.0, 0.0, 0.0, 0.5],    // Square with high square
    [0.5, 0.0, 0.0, 0.0, 0.0, 0.5],    // Saw with high square
    [0.33, 0.0, 0.0, 0.33, 0.0, 0.33], // Saws and squares
    [0.33, 0.0, 0.33, 0.0, 0.33, 0.0], // Full saw
    [0.5, 0.0, 0.5, 0.0, 0.0, 0.0],    // Two saws
    [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],    // Saw
    [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],    // Square
];
/// How far `timbre` and `morph` glide toward their target per block at
/// 48 kHz, as a one-pole coefficient.
const SMOOTHING: f32 = 0.1;
const TUNED_RATE: f32 = 48_000.0;
/// `aux` (the root alone) is brought up to about the chord's level.
const ROOT_GAIN: f32 = 3.0;

pub(in crate::modules::macro_oscillator) struct ChordsEngine {
    voices: [StringSynthOscillator; VOICES],
    bank: ChordBank,
    morph: f32,
    timbre: f32,
    /// Whether a block has played: the first snaps `morph` and `timbre`
    /// to their targets rather than sweeping up from zero.
    started: bool,
}

impl ChordsEngine {
    pub fn new() -> Self {
        Self {
            voices: std::array::from_fn(|_| StringSynthOscillator::new()),
            bank: ChordBank::new(),
            morph: 0.0,
            timbre: 0.0,
            started: false,
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let smoothing = if self.started {
            (SMOOTHING * TUNED_RATE * p.sample_period).min(1.0)
        } else {
            1.0
        };
        self.started = true;
        self.morph += (p.morph - self.morph) * smoothing;
        self.timbre += (p.timbre - self.timbre) * smoothing;

        // The chord changes once on a write, rather than passing through
        // every chord on the way.
        self.bank.select(p.harmonics_target);
        self.bank.build(p.note, p.key, p.scale);
        let voicing = self.bank.voice(self.timbre);
        let registration = registration(self.morph);

        out.fill(0.0);
        aux.fill(0.0);
        let f0 = p.note_to_frequency(p.note);
        for (k, voice) in self.voices.iter_mut().enumerate() {
            let level = voicing.levels[k];
            let frequency = f0 * voicing.ratios[k];
            // Fades out the top octave of the machine's range.
            let gain = level * (4.0 - frequency * 32.0).clamp(0.0, 1.0);
            // A voice that has faded to silence costs nothing; one that
            // has just gone silent still ramps down.
            if gain == 0.0 && voice.is_silent() {
                continue;
            }
            let destination = if voicing.root_mask & 1 << k != 0 {
                &mut *aux
            } else {
                &mut *out
            };
            voice.render(frequency, &registration, gain, destination);
        }
        for (o, a) in out.iter_mut().zip(aux.iter_mut()) {
            *o += *a;
            *a *= ROOT_GAIN;
        }
    }
}

/// The stop levels for a `morph` of 0 to 1, between neighbouring
/// registrations.
fn registration(morph: f32) -> [f32; STOPS] {
    let position = morph.clamp(0.0, 1.0) * (REGISTRATIONS.len() as f32 - 1.001);
    let integral = position as usize;
    let fractional = position - integral as f32;
    let (a, b) = (&REGISTRATIONS[integral], &REGISTRATIONS[integral + 1]);
    let mut stops = [0.0; STOPS];
    for (i, stop) in stops.iter_mut().take(6).enumerate() {
        *stop = a[i] + (b[i] - a[i]) * fractional;
    }
    stops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::macro_oscillator::engine::ENGINE_BLOCK;

    #[test]
    fn a_harmonics_write_changes_the_chord_once() {
        let mut engine = ChordsEngine::new();
        let mut p = EngineParameters {
            note: 48.0,
            sample_period: 1.0 / 48_000.0,
            harmonics: 0.1,
            harmonics_target: 0.1,
            timbre: 0.5,
            morph: 0.5,
            accent: 0.8,
            key: 0,
            scale: 0,
        };
        let (mut out, mut aux) = ([0.0; ENGINE_BLOCK], [0.0; ENGINE_BLOCK]);
        engine.render(&p, &mut out, &mut aux);
        let mut shapes = vec![engine.bank.shape()];
        // The module glides `harmonics` toward the write over 40 blocks.
        p.harmonics_target = 0.9;
        for block in 1..=40 {
            p.harmonics = 0.1 + 0.8 * block as f32 / 40.0;
            engine.render(&p, &mut out, &mut aux);
            if shapes.last() != Some(&engine.bank.shape()) {
                shapes.push(engine.bank.shape());
            }
        }
        assert_eq!(shapes.len(), 2, "the chord went {shapes:?}");
    }
}
