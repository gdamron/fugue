//! The synthesis engines the `model` control chooses between.

mod additive;
mod chord_bank;
mod chord_waves;
mod chords;
mod drums;
mod dust;
mod fm;
mod grain_formant;
mod limiter;
mod modal;
mod noise;
mod six_op;
mod speech;
mod string;
mod tables;
mod virtual_analog;
mod wave_shapes;
mod waveshaping;
mod wavetable;
mod wavetables;

use self::additive::AdditiveEngine;
use self::chords::ChordsEngine;
use self::drums::{BassDrumEngine, HiHatEngine, SnareDrumEngine};
use self::fm::FmEngine;
use self::grain_formant::GrainFormantEngine;
use self::limiter::Limiter;
use self::modal::ModalEngine;
use self::noise::{FilteredNoiseEngine, ParticleNoiseEngine};
use self::six_op::SixOpEngine;
use self::speech::SpeechEngine;
use self::string::StringEngine;
use self::virtual_analog::VirtualAnalogEngine;
use self::waveshaping::WaveshapingEngine;
use self::wavetable::WavetableEngine;
use super::engine::EngineParameters;

/// The models' names, in `model` position order. New engines are only
/// ever appended, so a saved position keeps meaning the same engine.
pub(super) const MODELS: &[&str] = &[
    "virtual_analog",
    "waveshaping",
    "fm",
    "grain_formant",
    "additive",
    "chords",
    "speech",
    "wavetable",
    "six_op_fm",
    "string",
    "modal",
    "bass_drum",
    "snare_drum",
    "hi_hat",
    "filtered_noise",
    "particle_noise",
];

/// How many engines there are.
pub(super) const MODEL_COUNT: usize = MODELS.len();

/// Each engine's output levels (`audio`, `aux`), so the models sit at
/// about the same loudness. Speech limits its own `audio`; the wavetables
/// are normalised, and peak below 1, when they are built; a negative level
/// is a limiter's drive, for an engine whose loudness swings widely with
/// its settings.
const LEVELS: [(f32, f32); MODEL_COUNT] = [
    (0.8, 0.8),
    (0.7, 0.6),
    (0.6, 0.6),
    (0.7, 0.6),
    (0.8, 0.8),
    (0.8, 0.8),
    (1.0, 0.8),
    (1.0, 1.0),
    (1.0, 1.0),
    (-1.0, 0.8),
    (-1.0, 0.8),
    (0.8, 0.8),
    (0.8, 0.8),
    (0.8, 0.8),
    (-1.0, -1.0),
    (-2.0, 1.0),
];

/// Whether each engine shapes its own sound over time when struck by a
/// trigger (a string, a body, a drum), so the low-pass gate leaves it be.
pub(super) const ENVELOPED: [bool; MODEL_COUNT] = [
    false, false, false, false, false, false, false, false, true, true, true, true, true, true,
    false, false,
];

/// Every engine, built up front so switching allocates nothing.
pub(super) struct Engines {
    virtual_analog: VirtualAnalogEngine,
    waveshaping: WaveshapingEngine,
    fm: FmEngine,
    grain_formant: GrainFormantEngine,
    additive: AdditiveEngine,
    chords: ChordsEngine,
    speech: SpeechEngine,
    wavetable: WavetableEngine,
    six_op: SixOpEngine,
    string: StringEngine,
    modal: ModalEngine,
    bass_drum: BassDrumEngine,
    snare_drum: SnareDrumEngine,
    hi_hat: HiHatEngine,
    filtered_noise: FilteredNoiseEngine,
    particle_noise: ParticleNoiseEngine,
    /// The `audio` and `aux` limiters of each engine with a negative level.
    limiters: [[Limiter; 2]; MODEL_COUNT],
}

impl Engines {
    /// Builds every engine for `sample_rate`, and the tables they share.
    pub fn new(sample_rate: u32) -> Self {
        tables::force();
        wavetables::force();
        Self {
            virtual_analog: VirtualAnalogEngine::new(),
            waveshaping: WaveshapingEngine::new(),
            fm: FmEngine::new(sample_rate),
            grain_formant: GrainFormantEngine::new(sample_rate),
            additive: AdditiveEngine::new(),
            chords: ChordsEngine::new(),
            speech: SpeechEngine::new(sample_rate),
            wavetable: WavetableEngine::new(),
            six_op: SixOpEngine::new(),
            string: StringEngine::new(sample_rate),
            modal: ModalEngine::new(sample_rate),
            bass_drum: BassDrumEngine::new(sample_rate),
            snare_drum: SnareDrumEngine::new(sample_rate),
            hi_hat: HiHatEngine::new(sample_rate),
            filtered_noise: FilteredNoiseEngine::new(sample_rate),
            particle_noise: ParticleNoiseEngine::new(sample_rate),
            limiters: std::array::from_fn(|_| {
                [Limiter::new(sample_rate), Limiter::new(sample_rate)]
            }),
        }
    }

    /// Renders one block of engine `model` (below [`MODEL_COUNT`]) into
    /// `out` and `aux`, at its output levels.
    pub fn render(&mut self, model: usize, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        match model {
            0 => self.virtual_analog.render(p, out, aux),
            1 => self.waveshaping.render(p, out, aux),
            2 => self.fm.render(p, out, aux),
            3 => self.grain_formant.render(p, out, aux),
            4 => self.additive.render(p, out, aux),
            5 => self.chords.render(p, out, aux),
            6 => self.speech.render(p, out, aux),
            7 => self.wavetable.render(p, out, aux),
            8 => self.six_op.render(p, out, aux),
            9 => self.string.render(p, out, aux),
            10 => self.modal.render(p, out, aux),
            11 => self.bass_drum.render(p, out, aux),
            12 => self.snare_drum.render(p, out, aux),
            13 => self.hi_hat.render(p, out, aux),
            14 => self.filtered_noise.render(p, out, aux),
            _ => self.particle_noise.render(p, out, aux),
        }
        let (out_level, aux_level) = LEVELS[model];
        let [out_limiter, aux_limiter] = &mut self.limiters[model];
        for (block, level, limiter) in
            [(out, out_level, out_limiter), (aux, aux_level, aux_limiter)]
        {
            if level < 0.0 {
                limiter.process(-level, block);
            } else {
                block.iter_mut().for_each(|s| *s *= level);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The models that shape their own envelope, by name, so a model added
    /// out of step with `ENVELOPED` is caught rather than gated by mistake.
    const STRUCK: &[&str] = &[
        "six_op_fm",
        "string",
        "modal",
        "bass_drum",
        "snare_drum",
        "hi_hat",
    ];

    #[test]
    fn exactly_the_struck_models_shape_their_own_envelope() {
        for (model, name) in MODELS.iter().enumerate() {
            assert_eq!(ENVELOPED[model], STRUCK.contains(name), "{name}");
        }
    }

    /// A struck model rings out to silence, never to subnormal numbers:
    /// a decay in `f32` sticks in them unless it is flushed.
    #[test]
    fn every_model_rings_out_to_zero_not_subnormals() {
        use crate::modules::macro_oscillator::engine::ENGINE_BLOCK;
        for (model, name) in MODELS.iter().enumerate() {
            for (note, shape) in [
                (36.0, 0.2),
                (60.0, 0.5),
                (84.0, 0.9),
                (48.0, 0.0),
                (48.0, 1.0),
            ] {
                let mut engines = Engines::new(48_000);
                let mut p = EngineParameters {
                    note,
                    sample_period: 1.0 / 48_000.0,
                    harmonics: 1.0 - shape,
                    harmonics_target: 1.0 - shape,
                    timbre: 0.5,
                    morph: shape,
                    accent: 0.8,
                    trigger: true,
                    untriggered: false,
                    trigger_held: false,
                    key: 0,
                    scale: 0,
                };
                let (mut out, mut aux) = ([0.0; ENGINE_BLOCK], [0.0; ENGINE_BLOCK]);
                // Twenty seconds: long enough for all but the most open
                // decays, which ring out through the same flushes.
                for block in 0..20 * 48_000 / ENGINE_BLOCK {
                    engines.render(model, &p, &mut out, &mut aux);
                    p.trigger = false;
                    let bad = out
                        .iter()
                        .chain(&aux)
                        .find(|v| **v != 0.0 && !v.is_normal());
                    assert!(
                        bad.is_none(),
                        "{name} at {note}/{shape}, block {block}: {bad:?}"
                    );
                }
            }
        }
    }
}
