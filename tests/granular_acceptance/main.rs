//! Acceptance tests for the `granular` module, written from the interface
//! spec alone. They drive only the public API (`ModuleRegistry`, `Module`,
//! `RenderEngine`) and measure what is heard: steps (clicks) at freeze,
//! unfreeze and mode changes, bounded output at full feedback, seamless
//! playback across the buffer's wrap point, grain onsets on clock divisions,
//! determinism, tape glide in the looping delay, and the declared control
//! surface.
//!
//! The `granular` type is built by name, so this suite compiles before the
//! module exists and fails at runtime until it does.
//!
//! Assumptions beyond the spec are marked "Assumption" on the test that makes
//! them. The 60 s feedback renders are long by design (a stability claim).
//! Run with `cargo test --release --test granular_acceptance`.

mod rig;

mod clock_sync;
mod definition;
mod feedback_bound;
mod glide;
mod step_continuity;
mod wrap;

pub use fugue::{ControlValue, GraphModule, Module, ModuleRegistry, RenderEngine};
pub use serde_json::{json, Value};
