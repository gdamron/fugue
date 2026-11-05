//! Acceptance tests for block-wise feedback: a feedback group whose every
//! cycle passes through a module that declares a minimum delay (the `delay`
//! atom) runs in chunks of that delay, so the loop has exactly the delay's
//! length; otherwise it falls back to per-sample scheduling (loop = delay + 1
//! sample per pass). Written from the issue and the interface note alone; they
//! drive only the public API (`RenderEngine`, `InventionBuilder`,
//! `ModuleRegistry`, `Module`) and measure what is heard: echo positions,
//! bit-exactness, steps.
//!
//! Echoes are located by the rising edges of a clock gate (0.5 high, edges at
//! 0.02), so positions are exact in samples. The tests that flip the
//! process-global kill switch are in `feedback_chunks_kill_switch_acceptance`.
//!
//! Not written here: allocation-/lock-freedom on the audio thread (the module
//! catalog test covers it) and the existing `invention::` suite passing with
//! the cut on (it runs as it is).
//!
//! Run with `cargo test --test feedback_chunks_acceptance`.

mod rig;

mod atom;
mod development;
mod edits;
mod equivalence;
mod fallback;
mod reload;
mod spacing;

pub use fugue::{GraphModule, Module, ModuleRegistry, RenderEngine};
pub use rig::*;
pub use serde_json::{json, Value};
