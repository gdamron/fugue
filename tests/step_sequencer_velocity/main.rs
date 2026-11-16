//! The step sequencer's `velocity` output, through the public API
//! (`RenderEngine`, `InventionBuilder`, `RunningInvention::take_notes`):
//!
//! - It carries the velocity of the most recently started attack (a
//!   principal or a grace note) and changes only as one starts, on the
//!   sample the gate rises, holding through the note, its release, rests
//!   and `held` steps; 1.0 before any note and for a step that declares none.
//! - Each grace note sounds at 0.8 times its step's velocity, the principal
//!   at the step's own, under either `grace_placement`.
//! - Unconnected, stored velocities change nothing that is heard, and every
//!   first-party example that plays a step sequencer still sounds.
//! - Connected to a voice's `velocity` input, a quieter step is quieter,
//!   without a click at the change.
//! - Played-note reports carry the step's velocity.

#[path = "../support/mod.rs"]
#[allow(dead_code)]
mod shared;

mod rig;

mod notes;
mod output;
mod unconnected;
mod voice_level;
