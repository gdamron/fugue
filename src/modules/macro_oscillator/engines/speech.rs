//! Ported from Mutable Instruments' `plaits/dsp/speech/naive_speech_synth.cc`
//! (MIT, Copyright 2016 Emilie Gillet), `plaits/dsp/engine/speech_engine.cc`
//! (MIT, Copyright 2016 Emilie Gillet) and `stmlib/dsp/limiter.h` (MIT,
//! Copyright 2015 Emilie Gillet), adapted to Fugue's block processing.
//! See THIRD_PARTY_NOTICES.md.
//!
//! Vowels from synthesizer parts: a glottal pulse through five resonant
//! band-pass formants. `morph` moves through the vowels, `timbre` through
//! the vocal registers from low to high voice, and `harmonics` shifts
//! every formant up to an octave either way, from a larger to a smaller
//! voice. A trigger drops the pitch and first formant an octave for 50 ms,
//! a consonant-like click. `aux` is the bare glottal pulse.

use crate::dsp::Svf;
use crate::modules::macro_oscillator::engine::{EngineParameters, ENGINE_BLOCK};
use crate::modules::macro_oscillator::oscillators::ImpulseTrainOscillator;

const FORMANTS: usize = 5;
const VOWELS: usize = 5;
const REGISTERS: usize = 5;
/// Each formant as (pitch in semitones, MIDI-style, and level out of
/// 256), per vowel and register.
type Formants = [(u8, u8); FORMANTS];
const PHONEMES: [[Formants; REGISTERS]; VOWELS] = [
    [
        [(74, 255), (83, 114), (97, 90), (98, 90), (100, 25)],
        [(75, 255), (84, 128), (100, 114), (101, 101), (103, 20)],
        [(76, 255), (85, 128), (100, 18), (102, 16), (104, 3)],
        [(79, 255), (85, 161), (101, 25), (104, 4), (110, 0)],
        [(79, 255), (85, 128), (101, 6), (106, 25), (110, 0)],
    ],
    [
        [(67, 255), (91, 64), (98, 90), (101, 64), (102, 32)],
        [(67, 255), (92, 51), (99, 64), (103, 51), (105, 25)],
        [(69, 255), (93, 51), (100, 32), (102, 25), (103, 25)],
        [(67, 255), (91, 16), (100, 8), (103, 4), (110, 0)],
        [(65, 255), (95, 25), (101, 45), (105, 2), (110, 0)],
    ],
    [
        [(59, 255), (92, 8), (99, 40), (102, 20), (104, 10)],
        [(61, 255), (94, 45), (101, 32), (103, 25), (105, 8)],
        [(60, 255), (93, 16), (101, 16), (104, 4), (105, 4)],
        [(65, 255), (92, 25), (100, 8), (105, 4), (110, 0)],
        [(60, 255), (96, 64), (101, 12), (106, 12), (110, 1)],
    ],
    [
        [(67, 255), (78, 72), (98, 22), (99, 25), (101, 2)],
        [(67, 255), (79, 80), (99, 64), (101, 64), (102, 12)],
        [(68, 255), (79, 80), (100, 12), (102, 20), (103, 5)],
        [(69, 255), (79, 90), (101, 40), (104, 10), (110, 0)],
        [(69, 255), (79, 72), (101, 20), (106, 20), (110, 0)],
    ],
    [
        [(65, 255), (74, 25), (98, 6), (100, 10), (101, 4)],
        [(65, 255), (74, 25), (100, 36), (101, 51), (103, 12)],
        [(66, 255), (75, 25), (100, 18), (102, 8), (104, 5)],
        [(63, 255), (77, 64), (99, 8), (104, 2), (110, 0)],
        [(63, 255), (77, 40), (100, 4), (106, 2), (110, 0)],
    ],
];
const FORMANT_Q: f32 = 20.0;
/// How long a trigger's click lasts, in seconds.
const CLICK_SECONDS: f32 = 0.05;
/// The formant shift `harmonics` reaches at either end, in semitones.
const SHIFT_RANGE: f32 = 12.0;
/// The limiter's gain into it, its attack and release per sample at
/// 48 kHz, and its output trim. The drive is four times the original's,
/// so the voice sits at the other models' loudness; the limiter holds its
/// peaks to the trim.
const LIMITER_DRIVE: f32 = 2.8;
const LIMITER_ATTACK: f32 = 0.05;
const LIMITER_RELEASE: f32 = 0.000_02;
const LIMITER_TRIM: f32 = 0.8;
/// How far `harmonics`, `timbre` and `morph` glide toward their target
/// per block at 48 kHz, as a one-pole coefficient, so a jump in a control
/// moves the formants rather than stepping them.
const SMOOTHING: f32 = 0.1;
const TUNED_RATE: f32 = 48_000.0;

/// The inputs the formant filters were last set for, so unchanged
/// controls cost no `exp2` or `tan`.
type Setting = (u32, u32, u32, bool);

pub(in crate::modules::macro_oscillator) struct SpeechEngine {
    pulse: ImpulseTrainOscillator,
    coloration: Svf,
    formants: [Svf; FORMANTS],
    levels: [f32; FORMANTS],
    /// The formant levels the last block ended on, ramped from per sample.
    previous_levels: [f32; FORMANTS],
    /// `harmonics`, `timbre` and `morph`, smoothed; `None` before the
    /// first block, which snaps to its targets.
    macros: Option<[f32; 3]>,
    setting: Option<Setting>,
    click_frames: usize,
    click_length: usize,
    peak: f32,
    attack: f32,
    release: f32,
}

impl SpeechEngine {
    pub fn new(sample_rate: u32) -> Self {
        let rate = sample_rate.max(1) as f32;
        // The per-sample coefficients, moved to this rate.
        let per_sample = |c: f32| 1.0 - (1.0 - c).powf(48_000.0 / rate);
        Self {
            pulse: ImpulseTrainOscillator::new(),
            coloration: Svf::new(sample_rate, 800.0, 0.5),
            formants: std::array::from_fn(|_| Svf::new(sample_rate, 1_000.0, FORMANT_Q)),
            levels: [0.0; FORMANTS],
            previous_levels: [0.0; FORMANTS],
            macros: None,
            setting: None,
            click_frames: 0,
            click_length: (CLICK_SECONDS * rate) as usize,
            peak: 0.5,
            attack: per_sample(LIMITER_ATTACK),
            release: per_sample(LIMITER_RELEASE),
        }
    }

    pub fn render(&mut self, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        if p.trigger {
            self.click_frames = self.click_length;
        }
        self.click_frames -= self.click_frames.min(out.len());
        let click = self.click_frames > 0;
        let mut f0 = p.note_to_frequency(p.note);
        if click {
            f0 *= 0.5;
        }

        // The glottal pulse, coloured.
        let mut pulse = [0.0; ENGINE_BLOCK];
        let pulse = &mut pulse[..out.len()];
        self.pulse.render(f0, pulse);
        for x in pulse.iter_mut() {
            *x = 4.0 * self.coloration.process(*x).bandpass;
        }

        let targets = [p.harmonics, p.timbre, p.morph];
        let first = self.macros.is_none();
        let macros = self.macros.get_or_insert(targets);
        let smoothing = (SMOOTHING * TUNED_RATE * p.sample_period).min(1.0);
        for (m, t) in macros.iter_mut().zip(targets) {
            *m += (t - *m) * smoothing;
        }
        let [harmonics, timbre, morph] = *macros;
        self.set_formants(morph, timbre, harmonics, click);
        if first {
            self.previous_levels = self.levels;
        }
        out.fill(0.0);
        let frames = out.len().max(1) as f32;
        for ((filter, &level), previous) in self
            .formants
            .iter_mut()
            .zip(&self.levels)
            .zip(self.previous_levels.iter_mut())
        {
            let step = (level - *previous) / frames;
            let mut gain = *previous;
            for (o, &x) in out.iter_mut().zip(pulse.iter()) {
                gain += step;
                *o += filter.process(x).bandpass * gain;
            }
            *previous = level;
        }
        aux.copy_from_slice(pulse);

        for o in out.iter_mut() {
            let s = *o * LIMITER_DRIVE;
            let error = s.abs() - self.peak;
            self.peak += error
                * if error > 0.0 {
                    self.attack
                } else {
                    self.release
                };
            let gain = if self.peak <= 1.0 {
                1.0
            } else {
                1.0 / self.peak
            };
            *o = s * gain * LIMITER_TRIM;
        }
    }

    /// Tunes the formants for a vowel and register (0 to 1 each) and a
    /// shift, between the neighbouring table entries.
    fn set_formants(&mut self, vowel: f32, register: f32, shift: f32, click: bool) {
        let setting = (vowel.to_bits(), register.to_bits(), shift.to_bits(), click);
        if self.setting == Some(setting) {
            return;
        }
        self.setting = Some(setting);
        let split = |x: f32, n: usize| {
            let x = x.clamp(0.0, 1.0) * (n as f32 - 1.001);
            (x as usize, x - x.floor())
        };
        let (v, vf) = split(vowel, VOWELS);
        let (r, rf) = split(register, REGISTERS);
        let shift = (shift.clamp(0.0, 1.0) - 0.5) * 2.0 * SHIFT_RANGE;
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let filters = self.formants.iter_mut().zip(self.levels.iter_mut());
        for (i, (filter, gain)) in filters.enumerate() {
            let at = |v: usize, r: usize| {
                let (pitch, level) = PHONEMES[v][r][i];
                (f32::from(pitch), f32::from(level))
            };
            let (v0r0, v0r1, v1r0, v1r1) = (at(v, r), at(v, r + 1), at(v + 1, r), at(v + 1, r + 1));
            let pitch = lerp(lerp(v0r0.0, v0r1.0, rf), lerp(v1r0.0, v1r1.0, rf), vf);
            let level = lerp(lerp(v0r0.1, v0r1.1, rf), lerp(v1r0.1, v1r1.1, rf), vf);
            let mut hz = 440.0 * ((pitch.min(160.0) + shift - 69.0) / 12.0).exp2();
            if click && i == 0 {
                hz *= 0.5;
            }
            filter.set(hz, FORMANT_Q);
            *gain = level / 256.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(morph: f32) -> EngineParameters {
        EngineParameters {
            note: 45.0,
            sample_period: 1.0 / 48_000.0,
            harmonics: 0.5,
            harmonics_target: 0.5,
            timbre: 0.3,
            morph,
            accent: 0.8,
            trigger: false,
            untriggered: true,
            trigger_held: false,
            key: 0,
            scale: 0,
        }
    }

    /// The level of `x` near `hz`, by a single-bin DFT.
    fn level_at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, &v) in x.iter().enumerate() {
            let a = std::f32::consts::TAU * hz * i as f32 / 48_000.0;
            re += v * a.cos();
            im += v * a.sin();
        }
        (re * re + im * im).sqrt() / x.len() as f32
    }

    #[test]
    fn each_vowel_puts_its_first_formant_where_the_table_says() {
        // Vowels 0 and 2 at the low register: first formants near notes
        // 74.6 (about 680 Hz) and 59.8 (about 260 Hz), on a 110 Hz voice.
        let mut energy = Vec::new();
        for morph in [0.0, 0.5] {
            let mut engine = SpeechEngine::new(48_000);
            let (mut out, mut aux) = ([0.0; ENGINE_BLOCK], [0.0; ENGINE_BLOCK]);
            let mut x = Vec::with_capacity(24_000);
            for _ in 0..2_000 {
                engine.render(&params(morph), &mut out, &mut aux);
                x.extend_from_slice(&out);
            }
            let x = &x[4_800..];
            energy.push((level_at(x, 660.0), level_at(x, 220.0)));
        }
        assert!(energy[0].0 > 2.0 * energy[1].0, "{energy:?}");
        assert!(energy[1].1 > 2.0 * energy[0].1, "{energy:?}");
    }

    #[test]
    fn a_trigger_drops_the_pitch_an_octave_for_50_ms() {
        let pulses = |trigger: bool| {
            let mut engine = SpeechEngine::new(48_000);
            let (mut out, mut aux) = ([0.0; ENGINE_BLOCK], [0.0; ENGINE_BLOCK]);
            for _ in 0..400 {
                engine.render(&params(0.5), &mut out, &mut aux);
            }
            let mut x = Vec::new();
            for block in 0..180 {
                let mut p = params(0.5);
                p.trigger = trigger && block == 0;
                engine.render(&p, &mut out, &mut aux);
                x.extend_from_slice(&aux);
            }
            let peak = x.iter().fold(0.0f32, |m, v| m.max(*v));
            x.windows(2)
                .filter(|w| w[0] < 0.5 * peak && w[1] >= 0.5 * peak)
                .count()
        };
        // 45 ms at 110 Hz is five pulses; at 55 Hz, two or three.
        let (plain, clicked) = (pulses(false), pulses(true));
        assert!((4..=6).contains(&plain), "{plain}");
        assert!((2..=3).contains(&clicked), "{clicked}");
    }
}
