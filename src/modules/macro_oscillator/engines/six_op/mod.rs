//! The six-operator FM synthesizer behind the `six_op_fm` model: its
//! operators, algorithms, envelopes and LFO, the patch set, and the voices
//! that play it.

mod algorithms;
mod engine;
#[cfg(test)]
mod engine_tests;
mod envelope;
mod lfo;
mod operators;
mod patch;
mod patches;
mod patches_held;
mod patches_struck;
mod voice;
#[cfg(test)]
mod voice_tests;

pub(super) use self::engine::SixOpEngine;
