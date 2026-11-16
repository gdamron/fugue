//! A note followed by `held` steps sounds as one sustained note, through
//! the public API (`RenderEngine`, `RunningInvention::take_notes`, and the
//! `StepSequencer` module driven sample by sample):
//!
//! - The `gate` stays high on every sample from the note's attack until the
//!   last held step's gate ends, which is where a held step's gate ends
//!   today: one measured step duration from its edge, less a sample.
//! - A held step does not re-attack: an envelope on the gate does not
//!   retrigger, velocity and frequency hold, and the played-note report
//!   shows one note.
//! - Across the loop point a held step 0 continues the note at the end of
//!   the pattern; on the first pass it is silent. One-shot does not wrap.
//! - A note not followed by a held step is unchanged.
//! - The gate never sticks high: rests, reset, the end of a one-shot,
//!   pattern writes, tempo changes and a stopped clock all release it.
//! - Grace-note principals (either placement, deferred or not) hold through
//!   a following held step at the principal's velocity.

#[path = "../support/mod.rs"]
#[allow(dead_code)]
mod shared;

mod rig;

mod drive;
mod notes;
mod render;
