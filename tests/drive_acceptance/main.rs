//! Acceptance tests for the `drive` module, written from its behavior spec
//! alone. They drive only the public API (`ModuleRegistry`, `GraphModule`,
//! `Module`) and measure what is heard.
//!
//! Spec summary: module type `drive`, input and output `audio`; controls
//! `drive`, `shape` (soft, hard, fold, asymmetric, bits, rate), `pre_tone`,
//! `post_tone`, `bias`, `level`, `auto_level`, `mix`; config `oversampling`
//! of 1, 2 or 4. Criteria: 4x oversampling cuts the aliasing of a hard-clipped
//! 5 kHz sine by at least 30 dB; asymmetric shapes leave a mean below 1e-3
//! after 1 s; with auto gain a -12 dBFS sine keeps its RMS within 3 dB across
//! the drive range; output stays within +/-1. Shape changes and drive sweeps
//! must not click, and controls are declared.
//!
//! Assumptions beyond the spec are marked "Assumption" in the doc comment of
//! the test that makes them.

mod rig;

mod aliasing;
mod bounded;
mod click_free;
mod mean;
mod steady_level;

pub use fugue::{GraphModule, Module, ModuleRegistry};
pub use serde_json::{json, Value};
