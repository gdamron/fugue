//! The minimum delay a module guarantees, which lets a feedback loop
//! through it run in chunks instead of sample by sample.

/// The shortest delay a module guarantees from some of its inputs to all of
/// its outputs (see [`crate::Module::feedback_cut`]).
///
/// A feedback group whose every cycle passes through such a module, entering
/// it at an `absorbed` input, runs in chunks of at most `frames` frames
/// rather than one sample at a time: the module [`emit`]s a chunk from its
/// history, the rest of the loop runs block-wise, then the module
/// [`absorb`]s the chunk's input. The loop is then exactly as long as its
/// delays, with no extra sample per pass.
///
/// A module that declares a cut must give the same output, bit for bit,
/// from `emit(n)` then `absorb(n)` over pieces of at most `frames` frames
/// as from `process` over the same stretch.
///
/// [`emit`]: crate::Module::emit
/// [`absorb`]: crate::Module::absorb
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedbackCut {
    /// Every output frame depends on the `absorbed` inputs only at least
    /// this many frames earlier. At least 1.
    pub frames: usize,
    /// The input ports (indexes into [`crate::Module::inputs`]) that only
    /// `absorb` reads. `emit` reads every other input, so one fed from
    /// inside the loop puts the loop back to sample by sample.
    pub absorbed: &'static [usize],
}
