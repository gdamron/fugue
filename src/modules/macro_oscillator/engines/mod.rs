//! The synthesis engines the `model` control chooses between.

mod additive;
mod chord_bank;
mod chords;
mod fm;
mod grain_formant;
mod tables;
mod virtual_analog;
mod waveshaping;

use self::additive::AdditiveEngine;
use self::chords::ChordsEngine;
use self::fm::FmEngine;
use self::grain_formant::GrainFormantEngine;
use self::virtual_analog::VirtualAnalogEngine;
use self::waveshaping::WaveshapingEngine;
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
];

/// How many engines there are.
pub(super) const MODEL_COUNT: usize = MODELS.len();

/// Each engine's output levels (`audio`, `aux`), so the models sit at
/// about the same loudness.
const LEVELS: [(f32, f32); MODEL_COUNT] = [
    (0.8, 0.8),
    (0.7, 0.6),
    (0.6, 0.6),
    (0.7, 0.6),
    (0.8, 0.8),
    (0.8, 0.8),
];

/// Every engine, built up front so switching allocates nothing.
pub(super) struct Engines {
    virtual_analog: VirtualAnalogEngine,
    waveshaping: WaveshapingEngine,
    fm: FmEngine,
    grain_formant: GrainFormantEngine,
    additive: AdditiveEngine,
    chords: ChordsEngine,
}

impl Engines {
    /// Builds every engine for `sample_rate`, and the tables they share.
    pub fn new(sample_rate: u32) -> Self {
        tables::force();
        Self {
            virtual_analog: VirtualAnalogEngine::new(),
            waveshaping: WaveshapingEngine::new(),
            fm: FmEngine::new(sample_rate),
            grain_formant: GrainFormantEngine::new(sample_rate),
            additive: AdditiveEngine::new(),
            chords: ChordsEngine::new(),
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
            _ => self.chords.render(p, out, aux),
        }
        let (out_level, aux_level) = LEVELS[model];
        out.iter_mut().for_each(|s| *s *= out_level);
        aux.iter_mut().for_each(|s| *s *= aux_level);
    }
}
