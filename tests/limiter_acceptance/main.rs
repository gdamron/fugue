//! Acceptance tests for the `limiter` module (a stereo-linked lookahead
//! brickwall limiter), written from its spec alone: the issue's definition of
//! done and the pinned interface (ports `audio_left`/`audio_right` in and out,
//! controls `ceiling` dB, `release` s, read-only `reduction` dB, config
//! `lookahead` s rounded to whole samples). They drive only the public API
//! (`ModuleRegistry`, `GraphModule`, `Module`).
//!
//! Criteria: no sample exceeds the ceiling on +12 dB noise and sine bursts;
//! below the ceiling the output is the input delayed by exactly the lookahead
//! in samples; gain reduction is readable as the `reduction` control.
//!
//! Assumptions beyond the spec are marked "Assumption" in the doc comment of
//! the test that makes them. Run with `cargo test --release --test
//! limiter_acceptance`.

mod rig;

mod reduction;
mod transparency;

pub use fugue::{GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
