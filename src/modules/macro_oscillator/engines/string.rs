//! Ported from Mutable Instruments' `plaits/dsp/engine/string_engine.cc` and
//! `plaits/dsp/physical_modelling/string_voice.cc` (MIT, Copyright 2016
//! Emilie Gillet), adapted to Fugue: the string itself is the resonator's
//! (ported from the same family), kept in tune when stiff; the excitation
//! is DC-blocked before it reaches the string, where the reference blocks
//! DC inside the loop (which leaves low notes sharp); and a string that has
//! fallen silent stops running until it is plucked again. See
//! THIRD_PARTY_NOTICES.md.
//!
//! An inharmonic string, plucked by the trigger. `harmonics` sets its
//! structure: below the middle a curved bridge makes it buzz and sitar-like,
//! above it the string stiffens toward a piano wire and then to wobbling,
//! dispersive metal. `timbre` is brightness, `morph` how long it rings.
//! Three strings take turns, so a new note lets the last one ring on.
//! Un-triggered, the active string is bowed by a stream of dust. `aux` is
//! the excitation alone.

use super::dust::dust;
use crate::dsp::physical::{KarplusString, StringSettings};
use crate::dsp::{exp2, DcBlocker, Rng, Svf};
use crate::modules::macro_oscillator::engine::{EngineParameters, ENGINE_BLOCK};

/// How many strings take turns.
const STRINGS: usize = 3;
/// A string whose loop holds nothing louder than this has fallen silent.
const SILENT: f32 = 1.0e-7;
/// The excitation's DC blocker corner. Dust is one-signed, and the string
/// would hold its average as an offset.
const EXCITATION_DC_CUTOFF_HZ: f32 = 20.0;
/// How long ago, in seconds, the pitch a string keeps ringing at when the
/// next one takes over was read: a pitch that arrives a little before its
/// trigger then belongs to the new note, not the old one.
const PITCH_DELAY_SECONDS: f32 = 0.0035;
/// Blocks of pitch history kept, at least `PITCH_DELAY_SECONDS` at any
/// sample rate up to 192 kHz.
const PITCH_HISTORY: usize = 128;

pub(in crate::modules::macro_oscillator) struct StringEngine {
    sample_rate: f32,
    strings: [KarplusString; STRINGS],
    /// Each string's excitation filter and DC blocker.
    filters: [Svf; STRINGS],
    dc_blockers: [DcBlocker; STRINGS],
    /// Each string's pitch, as a fraction of the sample rate.
    pitches: [f32; STRINGS],
    /// The pitch of recent blocks, newest at `history_at`.
    history: [f32; PITCH_HISTORY],
    history_at: usize,
    /// How many blocks back the pitch kept by a string left ringing is.
    pitch_delay: usize,
    /// Blocks rendered, and the block each string was last struck in.
    blocks: usize,
    struck_at: [usize; STRINGS],
    /// How many frames of noise burst each string has left.
    noise_left: [usize; STRINGS],
    /// Whether each string is ringing (else it is skipped).
    ringing: [bool; STRINGS],
    active: usize,
    rng: Rng,
    excitation: [f32; ENGINE_BLOCK],
    pickup: [f32; ENGINE_BLOCK],
}

impl StringEngine {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1) as f32,
            strings: std::array::from_fn(|i| {
                KarplusString::new(sample_rate, true, 0x5717 + i as u64).with_tuned_stiffness()
            }),
            filters: std::array::from_fn(|_| Svf::new(sample_rate, 1_000.0, 0.5)),
            dc_blockers: std::array::from_fn(|_| {
                DcBlocker::with_cutoff(sample_rate, EXCITATION_DC_CUTOFF_HZ)
            }),
            pitches: [220.0 / sample_rate.max(1) as f32; STRINGS],
            history: [220.0 / sample_rate.max(1) as f32; PITCH_HISTORY],
            history_at: 0,
            blocks: 0,
            struck_at: [0; STRINGS],
            pitch_delay: ((PITCH_DELAY_SECONDS * sample_rate as f32 / ENGINE_BLOCK as f32).round()
                as usize)
                .clamp(1, PITCH_HISTORY - 1),
            noise_left: [0; STRINGS],
            ringing: [false; STRINGS],
            active: STRINGS - 1,
            rng: Rng::new(0x5717),
            excitation: [0.0; ENGINE_BLOCK],
            pickup: [0.0; ENGINE_BLOCK],
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        let size = out.len();
        out.fill(0.0);
        aux.fill(0.0);
        if p.trigger {
            // The string left ringing keeps the pitch from a moment ago, but
            // never one from before it was struck (a fast roll or flam).
            let since_struck = self.blocks.wrapping_sub(self.struck_at[self.active]);
            // The slot at `history_at` is the block before this one.
            let back = (self.pitch_delay - 1).min(since_struck.saturating_sub(1));
            let at = (self.history_at + PITCH_HISTORY - back) % PITCH_HISTORY;
            self.pitches[self.active] = self.history[at];
            self.active = (self.active + 1) % STRINGS;
            self.struck_at[self.active] = self.blocks;
        }
        self.blocks = self.blocks.wrapping_add(1);
        let f0 = p.note_to_frequency(p.note).min(0.49);
        self.pitches[self.active] = f0;
        self.history_at = (self.history_at + 1) % PITCH_HISTORY;
        self.history[self.history_at] = f0;
        for (string, pitch) in self.strings.iter_mut().zip(self.pitches) {
            string.glide_to(pitch, 1.0);
        }

        // How far the string's excitation and loss lean bright, and how long
        // it rings; a harder note is brighter and rings longer.
        let brightness = p.timbre * p.timbre;
        let density = brightness * brightness;
        let brightness = brightness + 0.25 * p.accent * (1.0 - brightness);
        let damping = p.morph + 0.25 * p.accent * (1.0 - p.morph);
        let structure = p.harmonics;
        let dispersion = if structure < 0.24 {
            (structure - 0.24) * 4.166
        } else if structure > 0.26 {
            (structure - 0.26) * 1.351_35
        } else {
            0.0
        };
        let settings = StringSettings {
            dispersion,
            // The string squares its brightness itself.
            brightness: brightness.sqrt(),
            damping,
            position: 0.5,
        };

        for i in 0..STRINGS {
            let active = i == self.active;
            let sustain = active && p.untriggered;
            let struck = active && p.trigger;
            let excitation = &mut self.excitation[..size];
            if struck || sustain {
                let f = 4.0 * f0;
                let cutoff = (f * exp2((brightness * (2.0 - brightness) - 0.5) * 6.0)).min(0.499);
                let q = if sustain { 1.0 } else { 0.5 };
                if struck {
                    self.noise_left[i] = (1.0 / f0) as usize;
                }
                self.filters[i].set(cutoff * self.sample_rate, q);
            }
            if sustain {
                let dust_f = 0.000_05 + 0.999_95 * density * density;
                for x in excitation.iter_mut() {
                    *x = dust(&mut self.rng, dust_f) * (8.0 - dust_f * 6.0) * p.accent;
                }
            } else {
                let burst = self.noise_left[i].min(size);
                self.noise_left[i] -= burst;
                for (k, x) in excitation.iter_mut().enumerate() {
                    *x = if k < burst { self.rng.bipolar() } else { 0.0 };
                }
            }
            let excited = sustain || struck || self.noise_left[i] > 0;
            if !(self.ringing[i] || excited || excitation.iter().any(|x| *x != 0.0)) {
                continue;
            }
            for (x, a) in excitation.iter_mut().zip(aux.iter_mut()) {
                *x = self.dc_blockers[i].process(self.filters[i].process(*x).lowpass);
                *a += *x;
            }
            let pickup = &mut self.pickup[..size];
            let level = self.strings[i].process(&settings, excitation, out, pickup);
            self.ringing[i] = level > SILENT || excited;
            if !self.ringing[i] {
                self.strings[i].reset();
                self.filters[i].reset();
                self.dc_blockers[i].reset();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn params(note: f32, trigger: bool, untriggered: bool) -> EngineParameters {
        EngineParameters {
            note,
            sample_period: 1.0 / RATE as f32,
            harmonics: 0.5,
            harmonics_target: 0.5,
            timbre: 0.5,
            morph: 0.5,
            accent: 0.8,
            trigger,
            untriggered,
            trigger_held: false,
            key: 0,
            scale: 0,
        }
    }

    fn play(engine: &mut StringEngine, p: &EngineParameters, blocks: usize) -> Vec<f32> {
        let (mut out, mut aux) = ([0.0; ENGINE_BLOCK], [0.0; ENGINE_BLOCK]);
        let mut sound = Vec::new();
        for _ in 0..blocks {
            engine.render(p, &mut out, &mut aux);
            sound.extend_from_slice(&out);
        }
        sound
    }

    /// Dust is one-signed; the string holds none of its average.
    #[test]
    fn bowed_by_dust_the_string_holds_no_offset() {
        let mut engine = StringEngine::new(RATE);
        let p = params(33.0, false, true);
        play(&mut engine, &p, 8_000);
        let x = play(&mut engine, &p, 8_000);
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        let rms = (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
        assert!(rms > 1e-3, "sounds at {rms}");
        assert!(mean.abs() < 0.02 * rms, "mean {mean} against rms {rms}");
    }

    /// Strings that have rung out stop running, and are exactly silent.
    #[test]
    fn rung_out_strings_fall_exactly_silent() {
        let mut engine = StringEngine::new(RATE);
        for note in [45.0, 52.0, 57.0, 64.0] {
            play(&mut engine, &params(note, true, false), 1);
            play(&mut engine, &params(note, false, false), 400);
        }
        let x = play(&mut engine, &params(64.0, false, false), 200_000);
        assert!(engine.ringing.iter().all(|r| !r), "{:?}", engine.ringing);
        assert!(x[x.len() - 1_000..].iter().all(|s| *s == 0.0));
    }

    /// A new pitch that arrives a block before its trigger belongs to the
    /// new note: the string left ringing keeps the old one.
    #[test]
    fn the_string_left_ringing_keeps_its_pitch() {
        let mut engine = StringEngine::new(RATE);
        play(&mut engine, &params(57.0, true, false), 1);
        play(&mut engine, &params(57.0, false, false), 100);
        let old = engine.active;
        play(&mut engine, &params(64.0, false, false), 1);
        play(&mut engine, &params(64.0, true, false), 1);
        let old_pitch = params(57.0, false, false).note_to_frequency(57.0);
        assert_ne!(engine.active, old);
        assert_eq!(engine.pitches[old], old_pitch);
    }

    /// Struck twice a block apart, the first string keeps the pitch it was
    /// struck at, not the one before it.
    #[test]
    fn a_flam_keeps_each_string_at_its_own_pitch() {
        let mut engine = StringEngine::new(RATE);
        play(&mut engine, &params(45.0, true, false), 1);
        play(&mut engine, &params(45.0, false, false), 100);
        play(&mut engine, &params(57.0, true, false), 1);
        let first = engine.active;
        play(&mut engine, &params(64.0, true, false), 1);
        let pitch = |note: f32| params(note, false, false).note_to_frequency(note);
        assert_eq!(engine.pitches[first], pitch(57.0));
        assert_eq!(engine.pitches[engine.active], pitch(64.0));
        assert_eq!(
            engine.pitches[engine.active],
            params(64.0, false, false).note_to_frequency(64.0)
        );
    }
}
