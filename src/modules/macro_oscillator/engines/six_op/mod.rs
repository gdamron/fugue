//! The six-operator FM synthesizer the `six_op_fm` model will play: its
//! operators and algorithms, envelopes and LFO, the patch set, and the
//! voice that plays a patch.

mod algorithms;
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
