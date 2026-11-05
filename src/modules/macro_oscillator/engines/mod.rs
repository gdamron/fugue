//! The synthesis engines the `model` control chooses between.

mod virtual_analog;

use self::virtual_analog::VirtualAnalogEngine;
use super::engine::EngineParameters;

/// The models' names, in `model` position order. New engines are only
/// ever appended, so a saved position keeps meaning the same engine.
pub(super) const MODELS: &[&str] = &["virtual_analog"];

/// How many engines there are.
pub(super) const MODEL_COUNT: usize = MODELS.len();

/// Each engine's output levels (`audio`, `aux`), so the models sit at
/// about the same loudness.
const LEVELS: [(f32, f32); MODEL_COUNT] = [(0.8, 0.8)];

/// Every engine, built up front so switching allocates nothing.
pub(super) struct Engines {
    virtual_analog: VirtualAnalogEngine,
}

impl Engines {
    pub fn new() -> Self {
        Self {
            virtual_analog: VirtualAnalogEngine::new(),
        }
    }

    /// Renders one block of engine `model` (below [`MODEL_COUNT`]) into
    /// `out` and `aux`, at its output levels.
    pub fn render(&mut self, model: usize, p: &EngineParameters, out: &mut [f32], aux: &mut [f32]) {
        self.virtual_analog.render(p, out, aux);
        let (out_level, aux_level) = LEVELS[model];
        out.iter_mut().for_each(|s| *s *= out_level);
        aux.iter_mut().for_each(|s| *s *= aux_level);
    }
}
